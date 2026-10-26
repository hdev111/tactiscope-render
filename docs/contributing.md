# Contributing

## Scope

This repository is the standalone renderer. Keep it free of:

- TactiScope app code, Electron/Next.js integration and project storage.
- Brand assets, marketing pages or editor UI.
- Browser rendering fallbacks.

New features should extend the render plan, the Rust renderer, the schema and
the tests.

## Checks

```bash
cargo fmt --check
cargo test --workspace
cargo clippy --workspace --all-targets
cargo run -p tactiscope-render-cli -- schema > /tmp/schema.json
```

The test suite includes a schema-sync test; update
`schema/render-plan.schema.json` with the CLI when changing plan types.

## Adding an element or effect

1. Add the plan type/field with serde attributes and `JsonSchema`.
2. Implement rendering/decoding in the renderer.
3. Update `schema/render-plan.schema.json`.
4. Add a Rust test and, when visual output changes, a fixture/golden check.
5. Update `docs/plan-schema.md`.

## Pull requests

- Keep changes focused.
- Include the commands and results you ran.
- Do not commit FFmpeg binaries, build artifacts or secrets.
- Preserve the MIT/FFmpeg notices.
