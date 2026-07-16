import type { BunSection, ContainerKind } from "./formats.js";
import type { ParsedOffsets } from "./offsets.js";

import { BUN_TRAILER, BUNFS_ROOT, BUNFS_ROOT_OLD, BUNFS_ROOT_WINDOWS } from "./constants.js";
import { InvalidExecutableError } from "./errors.js";

export interface BundledFile {
  path: string;
  contents: ArrayBuffer;
  sourcemap?: {
    version: 3;
    file: string;
    debugId?: string;
    mappings: string;
    sources: string[];
  };
}

export interface ModuleFormat {
  /** Size of each per-module metadata chunk (28/32/36/52 bytes). */
  chunkSize: number;
  /** New wire format inserts a NUL after each string (path/contents/sourcemap). */
  newFormat: boolean;
  /** V3/V4 chunks carry an absolute startOffset at +0; V1/V2 iterate sequentially. */
  hasAbsoluteOffsets: boolean;
}

/**
 * The fully-resolved module graph: everything needed to walk the modules, with
 * all invariants already checked. Built once up front by `buildModuleGraph`.
 */
export interface ModuleGraph {
  view: DataView;
  offsets: ParsedOffsets;
  format: ModuleFormat;
  /** Offset of the modules region within `view`. */
  modulesStart: number;
  /** Offset of the metadata region within `view` (right after the modules region). */
  metadataStart: number;
  moduleCount: number;
}

/**
 * Determine the on-disk module format from the container and Offsets struct.
 *   - structSize 32          → V4 (Bun 1.3.0+): 52-byte chunks, absolute offsets
 *   - section + structSize 24 → V3 (Bun 1.2.4+): 36-byte chunks, absolute offsets
 *   - appended + structSize 24 → V1/V2: 28 or 32-byte chunks (heuristic), sequential
 */
export function resolveModuleFormat(
  container: ContainerKind,
  offsets: ParsedOffsets,
  view: DataView,
  trailerOffset: number,
): ModuleFormat {
  if (offsets.structSize === 32) {
    return { chunkSize: 52, newFormat: true, hasAbsoluteOffsets: true };
  }

  if (container !== "append") {
    return { chunkSize: 36, newFormat: true, hasAbsoluteOffsets: true };
  }

  // V1/V2 appended: distinguish the 28-byte (new) and 32-byte (old) chunk layouts
  // by checking whether the payload size lines up with the metadata pointer.
  const structStart = trailerOffset - offsets.structSize;
  const payloadSize =
    view.getUint32(structStart - 20, true) + view.getUint32(structStart - 16, true);
  const newFormat = payloadSize + 1 === offsets.modulesPtrOffset;
  return { chunkSize: newFormat ? 28 : 32, newFormat, hasAbsoluteOffsets: false };
}

/**
 * Resolve the module format, compute region offsets, and assert every invariant
 * up front — so extraction can then run straight through and trust its inputs.
 */
export function buildModuleGraph(section: BunSection, offsets: ParsedOffsets): ModuleGraph {
  const { view, container } = section;
  const trailerOffset = view.byteLength - BUN_TRAILER.length;

  const format = resolveModuleFormat(container, offsets, view, trailerOffset);

  const modulesStart =
    view.byteLength - (offsets.offsetByteCount + offsets.structSize + BUN_TRAILER.length);
  const metadataStart = modulesStart + offsets.modulesPtrOffset;

  // Invariants — fail loudly rather than extracting garbage.
  if (modulesStart < 0) {
    throw new InvalidExecutableError(
      `Declared payload size (${offsets.offsetByteCount}) exceeds the section (${view.byteLength})`,
    );
  }
  if (offsets.modulesPtrLength % format.chunkSize !== 0) {
    throw new InvalidExecutableError(
      `Metadata length ${offsets.modulesPtrLength} is not a multiple of chunk size ${format.chunkSize}`,
    );
  }
  const moduleCount = offsets.modulesPtrLength / format.chunkSize;
  if (moduleCount === 0) {
    throw new InvalidExecutableError("Executable contains no bundled modules");
  }
  if (offsets.entryPointId >= moduleCount) {
    throw new InvalidExecutableError(
      `Entry point id ${offsets.entryPointId} is out of range for ${moduleCount} modules`,
    );
  }
  if (metadataStart + offsets.modulesPtrLength > trailerOffset) {
    throw new InvalidExecutableError("Module metadata extends past the Offsets struct");
  }

  return { view, offsets, format, modulesStart, metadataStart, moduleCount };
}

export interface ExtractModulesOptions {
  normaliseEntrypointFileName: boolean;
}

/** Walk the module graph and return every bundled file (entry point first). */
export function extractModules(graph: ModuleGraph, options: ExtractModulesOptions): BundledFile[] {
  const { view, offsets, format, modulesStart, metadataStart, moduleCount } = graph;
  const { chunkSize, newFormat, hasAbsoluteOffsets } = format;
  const decoder = new TextDecoder();

  // The library only ever wraps real ArrayBuffers, so this slice is an
  // ArrayBuffer (not SharedArrayBuffer); assert that for the type system.
  const modulesData = view.buffer.slice(
    view.byteOffset + modulesStart,
    view.byteOffset + metadataStart,
  ) as ArrayBuffer;

  const bundledFiles: BundledFile[] = [];
  let currentOffset = 0;

  for (let i = 0; i < moduleCount; i++) {
    const isEntrypoint = i === offsets.entryPointId;
    const metadataOffset = metadataStart + i * chunkSize;

    if (hasAbsoluteOffsets) {
      currentOffset = view.getUint32(metadataOffset, true);
    }

    const pathLength = view.getUint32(metadataOffset + 4, true);
    const contentsLength = view.getUint32(metadataOffset + 12, true);
    const sourcemapLength = view.getUint32(metadataOffset + 20, true);

    let path = decoder.decode(modulesData.slice(currentOffset, currentOffset + pathLength));
    if (options.normaliseEntrypointFileName && isEntrypoint) {
      path = path.replace(/\/[^\/\\]+$/, "/index.js");
    }
    path = removeBunfsRootFromPath(path);
    if (path[0] !== "/") {
      throw new InvalidExecutableError("Invalid path in bundled file in executable");
    }
    path = removeLeadingSlash(path);

    const contentsStart = currentOffset + pathLength + (newFormat ? 1 : 0);
    const contentsEnd = contentsStart + contentsLength;
    const contents = modulesData.slice(contentsStart, contentsEnd);

    let sourcemap: BundledFile["sourcemap"];
    if (sourcemapLength) {
      const sourcemapMappingsLength = view.getUint32(modulesStart + contentsEnd + 5, true);

      if (sourcemapMappingsLength) {
        const sourcemapSourcesCount = view.getUint32(modulesStart + contentsEnd + 1, true);

        const mappingsStart = contentsEnd + 9 + sourcemapSourcesCount * 16;
        const mappingsEnd = mappingsStart + sourcemapMappingsLength;

        const contentsEndData = decoder.decode(contents.slice(contents.byteLength - 49));
        const debugId = contentsEndData.match(/^\/\/# debugId=([a-fA-F0-9-]{12,})$/m)?.[1];
        // RegEx copied from the sourcemaps debug-id spec:
        // https://github.com/tc39/source-map/blob/main/proposals/debug-id.md#appendix-a-self-description-of-source-maps-and-javascript-files

        sourcemap = {
          version: 3,
          file: path,
          debugId,
          mappings: decoder.decode(modulesData.slice(mappingsStart, mappingsEnd)),
          sources: [],
        };

        let sourceStart = mappingsEnd;
        for (let j = 0; j < sourcemapSourcesCount; j++) {
          const sourcemapSourceLength = view.getUint32(
            modulesStart + contentsEnd + 13 + j * 8,
            true,
          );

          sourcemap.sources.push(
            decoder.decode(modulesData.slice(sourceStart, sourceStart + sourcemapSourceLength)),
          );

          sourceStart += sourcemapSourceLength;
        }
      }
    }

    const bundledFile: BundledFile = { path, contents, sourcemap };
    if (isEntrypoint) {
      bundledFiles.unshift(bundledFile);
    } else {
      bundledFiles.push(bundledFile);
    }

    currentOffset += pathLength + contentsLength + sourcemapLength + (newFormat ? 2 : 0);
  }

  return bundledFiles;
}

export function removeBunfsRootFromPath(path: string): string {
  if (path.startsWith(BUNFS_ROOT)) {
    return path.slice(BUNFS_ROOT.length);
  }
  if (path.startsWith(BUNFS_ROOT_OLD)) {
    return path.slice(BUNFS_ROOT_OLD.length);
  }
  const windowsRoot = BUNFS_ROOT_WINDOWS.exec(path);
  if (windowsRoot) {
    return path.slice(windowsRoot[0].length);
  }
  throw new Error(`Path does not start with Bun-fs root: ${path}`);
}

export function removeLeadingSlash(path: string): string {
  return path.replace(/^\/?(?:\.\/)?/, "");
}
