<div align="center">

<a href="https://github.com/ElvinEga/nodaclaw"><img src="https://raw.githubusercontent.com/moltis-org/moltis/main/website/favicon.svg" alt="Nodaclaw" width="64"></a>

# Nodaclaw — A Rust-native claw you can trust

One binary — sandboxed, secure, yours.

[![CI](https://github.com/ElvinEga/nodaclaw/actions/workflows/ci.yml/badge.svg)](https://github.com/ElvinEga/nodaclaw/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/ElvinEga/nodaclaw/graph/badge.svg)](https://codecov.io/gh/ElvinEga/nodaclaw)
[![CodSpeed](https://img.shields.io/endpoint?url=https://codspeed.io/badge.json&style=flat&label=CodSpeed)](https://codspeed.io/ElvinEga/nodaclaw)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.91%2B-orange.svg)](https://www.rust-lang.org)
[![Discord](https://img.shields.io/discord/1469505370169933837?color=5865F2&label=Discord&logo=discord&logoColor=white)](https://discord.gg/XnmrepsXp5)

[Installation](#installation) • [Comparison](#comparison) • [Architecture](#architecture--crate-map) • [Security](#security) • [Features](#features) • [How It Works](#how-it-works) • [Contributing](CONTRIBUTING.md)

</div>

---

Nodaclaw is a minimal rebrand of the Moltis codebase, forked from [moltis-org/moltis](https://github.com/moltis-org/moltis).

This fork keeps core functionality intact while the product identity shifts:

- `Nodaclaw` is the runtime and assistant shell.
- `Nodamem` will be the local-first memory engine.

The CLI, crate names, config paths, and environment variables still use `moltis` where that is currently part of the implementation.

**Secure by design** — Your keys never leave your machine. Every command runs in a sandboxed container, never on your host.

**Your hardware** — Runs on a Mac Mini, a Raspberry Pi, or any server you own. One Rust binary, no Node.js, no npm, no runtime.

**Full-featured** — Voice, memory, scheduling, Telegram, Discord, browser automation, MCP servers — all built-in. No plugin marketplace to get supply-chain attacked through.

**Auditable** — The agent loop + provider model fits in ~5K lines. The core (excluding the optional web UI) is ~196K lines across 46 modular crates you can audit independently, with 3,100+ tests and zero `unsafe` code\*.

## Installation

```bash
# One-liner install script (macOS / Linux)
curl -fsSL https://www.moltis.org/install.sh | sh

# macOS / Linux via Homebrew
brew install moltis-org/tap/moltis

# Docker (multi-arch: amd64/arm64)
docker pull ghcr.io/moltis-org/moltis:latest

# Or build from source
cargo install moltis --git https://github.com/moltis-org/moltis
```

## Comparison

| | OpenClaw | PicoClaw | NanoClaw | ZeroClaw | **Nodaclaw** |
|---|---|---|---|---|---|
| Language | TypeScript | Go | TypeScript | Rust | **Rust** |
| Agent loop | ~430K LoC | Small | ~500 LoC | ~3.4K LoC | **~5K LoC** (`runner.rs` + `model.rs`) |
| Full codebase | — | — | — | 1,000+ tests | **~124K LoC** (2,300+ tests) |
| Runtime | Node.js + npm | Single binary | Node.js | Single binary (3.4 MB) | **Single binary (44 MB)** |
| Sandbox | App-level | — | Docker | Docker | **Docker + Apple Container** |
| Memory safety | GC | GC | GC | Ownership | **Ownership, zero `unsafe`\*** |
| Auth | Basic | API keys | None | Token + OAuth | **Password + Passkey + API keys + Vault** |
| Voice I/O | Plugin | — | — | — | **Built-in (15+ providers)** |
| MCP | Yes | — | — | — | **Yes (stdio + HTTP/SSE)** |
| Hooks | Yes (limited) | — | — | — | **15 event types** |
| Skills | Yes (store) | Yes | Yes | Yes | **Yes (+ OpenClaw Store)** |
| Memory/RAG | Plugin | — | Per-group | SQLite + FTS | **SQLite + FTS + vector** |

\* `unsafe` is denied workspace-wide. The only exceptions are opt-in FFI wrappers behind the `local-embeddings` feature flag, not part of the core.

> [Full comparison with benchmarks →](https://docs.moltis.org/comparison.html)

## Architecture — Crate Map

**Core** (always compiled):

| Crate | LoC | Role |
|-------|-----|------|
| `moltis` (cli) | 4.0K | Entry point, CLI commands |
| `moltis-agents` | 9.6K | Agent loop, streaming, prompt assembly |
| `moltis-providers` | 17.6K | LLM provider implementations |
| `moltis-gateway` | 36.1K | HTTP/WS server, RPC, auth |
| `moltis-chat` | 11.5K | Chat engine, agent orchestration |
| `moltis-tools` | 21.9K | Tool execution, sandbox |
| `moltis-config` | 7.0K | Configuration, validation |
| `moltis-sessions` | 3.8K | Session persistence |
| `moltis-plugins` | 1.9K | Hook dispatch, plugin formats |
| `moltis-service-traits` | 1.3K | Shared service interfaces |
| `moltis-common` | 1.1K | Shared utilities |
| `moltis-protocol` | 0.8K | Wire protocol types |

**Optional** (feature-gated or additive):

| Category | Crates | Combined LoC |
|----------|--------|-------------|
| Web UI | `moltis-web` | 4.5K |
| GraphQL | `moltis-graphql` | 4.8K |
| Voice | `moltis-voice` | 6.0K |
| Memory | `moltis-memory`, `moltis-qmd` | 5.9K |
| Channels | `moltis-telegram`, `moltis-whatsapp`, `moltis-discord`, `moltis-msteams`, `moltis-channels` | 14.9K |
| Browser | `moltis-browser` | 5.1K |
| Scheduling | `moltis-cron`, `moltis-caldav` | 5.2K |
| Extensibility | `moltis-mcp`, `moltis-skills`, `moltis-wasm-tools` | 9.1K |
| Auth & Security | `moltis-auth`, `moltis-oauth`, `moltis-onboarding`, `moltis-vault` | 6.6K |
| Networking | `moltis-network-filter`, `moltis-tls`, `moltis-tailscale` | 3.5K |
| Provider setup | `moltis-provider-setup` | 4.3K |
| Import | `moltis-openclaw-import` | 7.6K |
| Apple native | `moltis-swift-bridge` | 2.1K |
| Metrics | `moltis-metrics` | 1.7K |
| Other | `moltis-projects`, `moltis-media`, `moltis-routing`, `moltis-canvas`, `moltis-auto-reply`, `moltis-schema-export`, `moltis-benchmarks` | 2.5K |

Use `--no-default-features --features lightweight` for constrained devices (Raspberry Pi, etc.).

## Security

- **Zero `unsafe` code\*** — denied workspace-wide; only opt-in FFI behind `local-embeddings` flag
- **Sandboxed execution** — Docker + Apple Container, per-session isolation
- **Secret handling** — `secrecy::Secret`, zeroed on drop, redacted from tool output
- **Authentication** — password + passkey (WebAuthn), rate-limited, per-IP throttle
- **SSRF protection** — DNS-resolved, blocks loopback/private/link-local
- **Origin validation** — rejects cross-origin WebSocket upgrades
- **Hook gating** — `BeforeToolCall` hooks can inspect/block any tool invocation

See [Security Architecture](https://docs.moltis.org/security.html) for details.

## Features

- **AI Gateway** — Multi-provider LLM support (OpenAI Codex, GitHub Copilot, Local), streaming responses, agent loop with sub-agent delegation, parallel tool execution
- **Communication** — Web UI, Telegram, Microsoft Teams, Discord, API access, voice I/O (8 TTS + 7 STT providers), mobile PWA with push notifications
- **Memory & Context** — Per-agent memory workspaces, embeddings-powered long-term memory, hybrid vector + full-text search, session persistence with auto-compaction, project context
- **Extensibility** — MCP servers (stdio + HTTP/SSE), skill system, 15 lifecycle hook events with circuit breaker, destructive command guard
- **Security** — Encryption-at-rest vault (XChaCha20-Poly1305 + Argon2id), password + passkey + API key auth, sandbox isolation, SSRF/CSWSH protection
- **Operations** — Cron scheduling, OpenTelemetry tracing, Prometheus metrics, cloud deploy (Fly.io, DigitalOcean), Tailscale integration

## How It Works

Nodaclaw is a **local-first AI gateway** — a single Rust binary that sits
between you and multiple LLM providers. Everything runs on your machine; no
cloud relay required.

```
┌─────────────┐  ┌─────────────┐  ┌─────────────┐
│   Web UI    │  │  Telegram   │  │  Discord    │
└──────┬──────┘  └──────┬──────┘  └──────┬──────┘
       │                │                │
       └────────┬───────┴────────┬───────┘
                │   WebSocket    │
                ▼                ▼
        ┌─────────────────────────────────┐
        │          Gateway Server         │
        │   (Axum · HTTP · WS · Auth)     │
        ├─────────────────────────────────┤
        │        Chat Service             │
        │  ┌───────────┐ ┌─────────────┐  │
        │  │   Agent   │ │    Tool     │  │
        │  │   Runner  │◄┤   Registry  │  │
        │  └─────┬─────┘ └─────────────┘  │
        │        │                        │
        │  ┌─────▼─────────────────────┐  │
        │  │    Provider Registry      │  │
        │  │  Multiple providers       │  │
        │  │  (Codex · Copilot · Local)│  │
        │  └───────────────────────────┘  │
        ├─────────────────────────────────┤
        │  Sessions  │ Memory  │  Hooks   │
        │  (JSONL)   │ (SQLite)│ (events) │
        └─────────────────────────────────┘
                       │
               ┌───────▼───────┐
               │    Sandbox    │
               │ Docker/Apple  │
               │  Container    │
               └───────────────┘
```

See [Quickstart](https://docs.moltis.org/quickstart.html) for gateway startup, message flow, sessions, and memory details.

## Getting Started

### Build & Run

Requires [just](https://github.com/casey/just) (command runner) and Node.js (for Tailwind CSS).

```bash
git clone https://github.com/ElvinEga/nodaclaw.git
cd nodaclaw
just build-css                  # Build Tailwind CSS for the web UI
just build-release              # Build in release mode
cargo run --release --bin moltis
```

For a full release build including WASM sandbox tools:

```bash
just build-release-with-wasm    # Builds WASM artifacts + release binary
cargo run --release --bin moltis
```

Open `https://moltis.localhost:3000`. On first run, a setup code is printed to
the terminal — enter it in the web UI to set your password or register a passkey.

The runtime branding is `Nodaclaw`, but the binary name remains `moltis` in this minimal pass.

Optional flags: `--config-dir /path/to/config --data-dir /path/to/data`

### Docker

```bash
# Docker / OrbStack
docker run -d \
  --name moltis \
  -p 13131:13131 \
  -p 13132:13132 \
  -p 1455:1455 \
  -v moltis-config:/home/moltis/.config/moltis \
  -v moltis-data:/home/moltis/.moltis \
  -v /var/run/docker.sock:/var/run/docker.sock \
  ghcr.io/moltis-org/moltis:latest
```

Open `https://localhost:13131` and complete the setup. For unattended Docker
deployments, set `MOLTIS_PASSWORD`, `MOLTIS_PROVIDER`, and `MOLTIS_API_KEY`
before first boot to skip the setup wizard. See [Docker docs](https://docs.moltis.org/docker.html)
for Podman, OrbStack, TLS trust, and persistence details.

### Cloud Deployment

| Provider | Deploy |
|----------|--------|
| DigitalOcean | [![Deploy to DO](https://www.deploytodo.com/do-btn-blue.svg)](https://cloud.digitalocean.com/apps/new?repo=https://github.com/moltis-org/moltis/tree/main) |

**Fly.io** (CLI):

```bash
fly launch --image ghcr.io/moltis-org/moltis:latest
fly secrets set MOLTIS_PASSWORD="your-password"
```

All cloud configs use `--no-tls` because the provider handles TLS termination.
See [Cloud Deploy docs](https://docs.moltis.org/cloud-deploy.html) for details.

## Star History

[![Star History Chart](https://api.star-history.com/svg?repos=ElvinEga/nodaclaw&type=date&legend=top-left)](https://www.star-history.com/#ElvinEga/nodaclaw&type=date&legend=top-left)

## License

MIT
