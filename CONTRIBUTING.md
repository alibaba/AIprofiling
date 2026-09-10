# Contributing to AIProf

Thanks for your interest in AIProf. Issues and pull requests are welcome
in both English and Chinese.

## Quick start for contributors

```bash
# clone
git clone https://github.com/alibaba/AIprofiling.git
cd AIprofiling

# build the Rust collection framework
cd agent/collection_framework && cargo build --release && cd ../..

# build the frontend
cd ui/webapp && npm install && npm run build && cd ../..

# smoke test the full stack (needs a GPU host)
cd deploy/docker && docker compose up -d --build && bash smoke.sh
```

See [`docs/deployment.md`](docs/deployment.md) for the full deployment
guide and [`docs/architecture.md`](docs/architecture.md) for the data
flow diagram.

## Reporting bugs

Open an issue with:

- What you tried to do
- What actually happened (error message, log lines — please redact secrets)
- Environment: OS, kernel version, NVIDIA driver + CUDA version, Docker
  version, whether client and server are co-located
- If reproducible: minimal steps or command line

For crashes inside the client (Rust) or the CUDA/CUPTI plugin, please
attach `RUST_LOG=debug` output and any core dumps you have.

## Proposing changes

- **Small fixes** (typos, one-line bugs): open a PR directly.
- **Feature work**: open an issue first to discuss the design. Non-trivial
  features benefit from an alignment round before code lands.
- **Vendored third-party changes**: see `agent/collection_framework/src/plugins/cuprof/VENDOR.md`
  for the re-sync procedure — do not edit vendored trees ad-hoc.

## Pull request checklist

- [ ] Rust code passes `cargo fmt` and `cargo clippy --release`
- [ ] Frontend passes `npm run lint` and `npm run build`
- [ ] `bash deploy/docker/smoke.sh` passes on your local host (if the change
      touches client, server, or the wire protocol)
- [ ] Commit messages are in English
- [ ] New features have at least one test; bug fixes include a regression test
- [ ] User-facing docs (README / `docs/`) are updated when behavior changes

## Coding conventions

- **Rust**: 4-space indent, `rustfmt` defaults, `anyhow` for application
  errors, `thiserror` for library errors. Log with `tracing`.
- **TypeScript / React**: follow existing patterns in `ui/webapp/src/`.
  Ant Design components for UI, plain fetch (no axios).
- **Python**: PEP 8, `black`-compatible formatting.
- **Node**: match the existing style in `server/dashboard/`.

## License

By contributing, you agree that your contributions will be licensed
under the Apache License 2.0 (see `LICENSE`).
