# Fuzz targets

Coverage-guided fuzzing harnesses for Miru's parse/decrypt paths. These require
a nightly toolchain + libFuzzer and are therefore **not run in the standard CI**
(which uses stable Rust). Run them locally during security review:

```bash
cargo install cargo-fuzz
cargo +nightly fuzz run fuzz_message_parse
cargo +nightly fuzz run fuzz_decrypt
```

## Targets

| Target | Invariant |
|--------|-----------|
| `fuzz_message_parse` | `serde_json::from_slice::<Msg>` never panics on arbitrary bytes |
| `fuzz_decrypt` | `SessionCipher::decrypt` never panics; garbage yields `Err` |

## CI coverage

The **same invariants** are also asserted by deterministic property tests that
DO run in standard CI, so a regression that breaks the no-panic guarantee is
caught even without nightly fuzzing:

- `crates/miru-common/src/tests.rs::message_parser_never_panics_on_garbage`
  — 10,000 seeded-random byte strings + hand-picked malformed JSON.
- `crates/miru-common/src/tests.rs::decrypt_never_panics_on_garbage`
  — 10,000 seeded-random ciphertexts + every boundary length 0..32.

Fuzzing explores far more of the input space; the property tests are the
always-on regression guard. Run the fuzzers before each release and after any
change to `message.rs` or `crypto.rs`.
