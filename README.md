# Touring

A high-performance, extensible media aggregation platform built with Rust and WebAssembly Component Model (WASM). Touring serves as both a standalone CLI tool and a robust backend engine for media aggregation applications, providing unified interfaces for searching and accessing manga and anime content through sandboxed plugins.

**CLI Mode:**

List available plugins:
```bash
./target/release/touring plugins
```

Search for manga:
```bash
./target/release/touring manga "one piece"
```

Filter plugins by name:
```bash
./target/release/touring plugins --name mangadex
```

**Backend Integration:**

Touring is designed to be embedded as a backend engine in frontend applications. The core functionality can be exposed through FFI bindings, REST APIs, or other integration methods suitable for your Dart/Flutter application.st and WebAssembly Component Model (WASM). Touring serves as both a standalone CLI tool and a robust backend engine for media aggregation applications, providing unified interfaces for searching and accessing manga and anime content through sandboxed plugins.

## Features

- 🔌 **Plugin Architecture**: Load and execute WASM plugins safely in a sandboxed environment
- 🔒 **Security**: WASI-based sandboxing ensures plugins cannot access host system resources
- 🚀 **Performance**: Built with Rust and Wasmtime for optimal performance
- 🌐 **Web Standards**: Uses WebAssembly Component Model for interoperability
- 📚 **Multi-Media**: Support for both manga and anime content aggregation
- 🛠️ **CLI Interface**: Standalone command-line interface for development and testing
- 🔧 **Backend Engine**: Rust core designed to power frontend applications (Dart/Flutter)
- 💾 **Data Management**: Plugin management, database storage, and configuration handling
- 📱 **Cross-Platform**: Backend suitable for mobile, desktop, and web applications

## Architecture

```
┌─────────────────┐    ┌──────────────────┐    ┌─────────────────┐
│  Frontend Apps  │───▶│                  │───▶│  WASM Plugins   │
│ (Dart/Flutter)  │    │  Touring Backend │    │                 │
└─────────────────┘    │                  │    └─────────────────┘
┌─────────────────┐    │  ┌─────────────┐ │           │
│   CLI Frontend  │───▶│  │   Plugin    │ │           ▼
└─────────────────┘    │  │  Manager    │ │    ┌──────────────┐
                       │  └─────────────┘ │    │  WASI/HTTP   │
                       │  ┌─────────────┐ │    │  Sandboxing  │
                       │  │  Database   │ │    └──────────────┘
                       │  │   & Config  │ │
                       │  └─────────────┘ │
                       └──────────────────┘
                              │
                              ▼
                       ┌──────────────┐
                       │  Wasmtime    │
                       │  Engine      │
                       └──────────────┘
```

## Quick Start

### Prerequisites

- Rust 1.75+ with 2024 edition support
- WebAssembly component toolchain (`wasm-tools`, `wit-bindgen`)

### Installation

1. Clone the repository:
```bash
git clone <repository-url>
cd touring
```

2. Build the project:
```bash
cargo build --release
```

3. Create a plugins directory and add your WASM plugins:
```bash
mkdir plugins
# Copy your .wasm plugin files to this directory
```

### Usage

List available plugins:
```bash
./target/release/touring plugins
```

Search for manga:
```bash
./target/release/touring manga "one piece"
```

Filter plugins by name:
```bash
./target/release/touring plugins --name mangadex
```

## Plugin Development

Touring uses the WebAssembly Component Model for plugins. Each plugin must implement the `source` world interface defined in `plugin-interface/wit/world.wit`.

### Interface Overview

```wit
world source {
  export fetchmangalist: func(query: string) -> list<media>;
  export fetchchapterimages: func(chapterid: string) -> list<string>;
  export fetchanimelist: func(query: string) -> list<media>;
  export fetchanimeepisodes: func(animeid: string) -> list<episode>;
  export fetchepisodestreams: func(episodeid: string) -> list<mediastream>;
}
```

### Creating a Plugin

1. Create a new Rust library project:
```bash
cargo new --lib my_plugin
cd my_plugin
```

2. Add dependencies to `Cargo.toml`:
```toml
[dependencies]
plugin-interface = { path = "../plugin-interface", features = ["guest"] }
wit-bindgen = "0.45"

[lib]
crate-type = ["cdylib"]

[[target]]
name = "wasm32-wasip2"
```

3. Implement the interface in `src/lib.rs`:
```rust
mod source;

pub struct PluginSource;

impl source::Guest for PluginSource {
    fn fetchmangalist(query: String) -> Vec<source::Media> {
        // Your implementation here
        vec![]
    }
    
    // Implement other required methods...
}

source::export!(PluginSource with_types_in crate::source);
```

4. Build the plugin:
```bash
cargo build --target wasm32-wasip2 --release
```

5. Precompile for the target runtime when shipping to mobile. Touring's iOS
    embedding runs Wasmtime's Pulley interpreter, so the `.cwasm` artifact must
    be produced with the `pulley64` target:
```bash
wasmtime compile your_plugin.wasm --target pulley64 -o your_plugin.cwasm
```
    or use the provided Makefiles with `WASMTIME_TARGETS="pulley64 aarch64-apple-ios"`.

6. Copy to plugins directory:
```bash
cp target/wasm32-wasip2/release/my_plugin.wasm ../touring/plugins/
```

### Security Considerations

Plugins run in a sandboxed WASI environment with the following restrictions:

- ✅ **Allowed**: HTTP requests (via WASI-HTTP)
- ✅ **Allowed**: Memory allocation within limits
- ✅ **Allowed**: Basic computation and string manipulation
- ❌ **Blocked**: File system access
- ❌ **Blocked**: Process spawning
- ❌ **Blocked**: Environment variable access
- ❌ **Blocked**: Network access outside HTTP

## Project Structure

```
touring/
├── src/
│   ├── main.rs           # CLI entry point
│   ├── cli.rs            # Command-line interface definitions
│   ├── plugins.rs        # Plugin management and execution
│   ├── database.rs       # Database operations (future)
│   ├── config.rs         # Configuration management (future)
│   └── backend.rs        # Backend API for frontend integration (future)
├── plugin-interface/     # Shared WIT interface definitions
│   ├── wit/
│   │   └── world.wit     # WebAssembly interface definition
│   └── src/
│       ├── lib.rs        # Rust bindings
│       ├── host.rs       # Host-side bindings
│       └── guest.rs      # Guest-side bindings
├── plugins/              # Directory for WASM plugin files
├── bindings/             # FFI bindings for frontend integration (future)
├── Cargo.toml           # Main workspace configuration
└── README.md            # This file
```

## Development

### Building

```bash
# Build the main application
cargo build

# Build with optimizations
cargo build --release

# Build the plugin interface
cd plugin-interface
cargo build

# Build as a library for backend integration
cargo build --lib --release
```

### Testing

```bash
# Run tests
cargo test

# Test with a specific plugin
./target/debug/touring manga "test query"
```

### Adding Commands

To add new CLI commands, modify `src/cli.rs`:

```rust
#[derive(Subcommand)]
pub enum Commands {
    // Existing commands...
    
    /// Your new command
    NewCommand {
        /// Command argument
        #[arg(short, long)]
        arg: String,
    },
}
```

Then handle it in `src/main.rs`.

### Backend Integration

For frontend applications, touring can be integrated as:

1. **Static Library**: Link directly with your application
2. **Dynamic Library**: Load at runtime with FFI
3. **REST API**: Expose functionality through HTTP endpoints
4. **IPC/Messaging**: Communicate through inter-process communication

The plugin management, database operations, and configuration handling are designed to be exposed through these integration methods.

## Dependencies

- **wasmtime**: WebAssembly runtime with component model support
- **wasmtime-wasi**: WASI implementation for sandboxing
- **wasmtime-wasi-http**: HTTP capabilities for plugins
- **clap**: Command-line argument parsing
- **tokio**: Async runtime for HTTP operations
- **serde**: Serialization framework for data handling
- **anyhow**: Error handling
- **sqlx** (future): Database operations
- **config** (future): Configuration management

## Contributing

1. Fork the repository
2. Create a feature branch
3. Make your changes
4. Add tests if applicable
5. Submit a pull request

## License

Copyright © 2025 5EUS

Permission is hereby granted, free of charge, to any person obtaining a copy of this software and associated documentation files (the “Software”), to deal in the Software without restriction, including without limitation the rights to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED “AS IS”, WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

## Examples

See the `plugins/` directory for example plugin implementations:
- `mangadx_plugin.wasm` - Example manga source plugin
- `nefarious_plugin.wasm` - Security testing plugin

For backend integration examples, see the `bindings/` directory (coming soon) for FFI examples and integration patterns with frontend frameworks.

## Troubleshooting

### Plugin Loading Issues

- Ensure plugins are compiled for `wasm32-wasip2` target
- Check that plugins implement all required interface methods
- Verify WASM files are in the `plugins/` directory

### Runtime Errors

- Check plugin compatibility with the current interface version
- Ensure sufficient memory is available for plugin execution
- Review plugin logs for specific error messages

### Performance

- Use `--release` builds for production
- Monitor memory usage with large plugin sets
- Consider async execution for I/O bound operations
