# Fuzzing

Fuzz targets for the two public entry points. The invariant under test is the
crate's no-panic contract: **any byte input returns `Ok`/`Err`, never panics
or aborts.** A crash here is a bug — almost certainly an unchecked read or
arithmetic overflow that should have been a typed `ExtractError`.

Requires a nightly toolchain and `cargo-fuzz`:

```sh
rustup toolchain install nightly
cargo install cargo-fuzz
```

Targets:

- `extract` — `extract_bundled_files` under both option settings.
- `version` — `get_executable_version` and `inspect_executable`.

Run one:

```sh
# Seed the corpus from the committed fixtures first (real payloads reach far
# deeper into the parser than random bytes do).
mkdir -p corpus/extract
cp ../src/lib/tests/fixtures/*.bin corpus/extract/

cargo +nightly fuzz run extract corpus/extract
cargo +nightly fuzz run version corpus/extract
```

Time-boxed run (what CI does — a smoke check, not a campaign):

```sh
cargo +nightly fuzz run extract corpus/extract -- -max_total_time=60
```

A finding is written to `fuzz/artifacts/<target>/`; reproduce it with
`cargo +nightly fuzz run <target> fuzz/artifacts/<target>/<crash-file>`.
