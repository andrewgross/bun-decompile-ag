//! The `bun-decompile debug <binary>` subcommand: a formatter over the
//! library's [`bun_decompile_ag::inspect_executable`]. Port of
//! `src/scripts/debug-binary.ts`, output kept line-compatible; `--payload`
//! is the port of `src/scripts/extract-section.ts`.

use std::path::Path;

use bun_decompile_ag::{extract_payload, inspect_executable};

pub fn run_debug(input_path: &Path, data: &[u8]) {
    let inspection = inspect_executable(data);

    println!("=== Binary Info ===");
    println!("File: {}", input_path.display());
    #[allow(clippy::cast_precision_loss)] // display only
    let mb = data.len() as f64 / 1024.0 / 1024.0;
    println!("Size: {} bytes ({mb:.1} MB)", data.len());

    println!("\n=== Executable Format ===");
    println!("Detected: {}", inspection.format);
    match inspection.section_offset {
        Some(offset) => println!("__BUN/.bun section offset: {offset}"),
        None => println!("__BUN/.bun section offset: (none — appended or absent)"),
    }

    println!("\n=== Bun Version ===");
    match &inspection.version {
        Ok(version) if version.revision.is_empty() => println!("{}", version.version),
        Ok(version) => println!("{} ({})", version.version, version.revision),
        Err(err) => println!("Not found: {err}"),
    }

    println!("\n=== Data Section ===");
    let payload = match inspection.payload {
        Ok(payload) => payload,
        Err(err) => {
            println!("FAILED: {err}");
            return;
        }
    };
    println!("Container: {}", payload.container);
    println!(
        "Payload size (ends at trailer): {} bytes",
        payload.payload_size
    );

    println!("\n=== Offsets Struct ===");
    let offsets = match payload.offsets {
        Ok(offsets) => offsets,
        Err(err) => {
            println!("FAILED: {err}");
            return;
        }
    };
    println!("structSize: {}", offsets.struct_size);
    println!("offsetByteCount: {}", offsets.offset_byte_count);
    println!("modulesPtrOffset: {}", offsets.modules_ptr_offset);
    println!("modulesPtrLength: {}", offsets.modules_ptr_length);
    println!("entryPointId: {}", offsets.entry_point_id);
    let hex: Vec<String> = offsets
        .struct_bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    println!("hex: {}", hex.join(" "));

    println!("\n=== Module Graph ===");
    match offsets.graph {
        Ok(graph) => {
            println!("Container: {}", payload.container);
            println!(
                "chunkSize: {} ({} modules)",
                graph.chunk_size, graph.module_count
            );
            println!(
                "newFormat: {}, absoluteOffsets: {}",
                graph.new_format, graph.has_absolute_offsets,
            );
            println!("modulesStart: {}", graph.modules_start);
        }
        Err(err) => println!("FAILED: {err}"),
    }
}

/// `--payload`: write the container-stripped payload, the layout of the
/// committed test fixtures (`build-fixtures.sh` makes them this way).
pub fn write_payload(data: &[u8], out_path: &Path) -> Result<(), String> {
    let blob = extract_payload(data).map_err(|err| err.to_string())?;
    std::fs::write(out_path, &blob).map_err(|err| err.to_string())?;
    println!("\n=== Payload ===");
    println!("Wrote {} bytes to {}", blob.len(), out_path.display());
    Ok(())
}
