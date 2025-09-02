# Touring Plugin Interface

This crate provides the WebAssembly plugin interface for the touring media aggregation library.

## Features

- WebAssembly Component Model support
- Host and guest feature flags for different use cases
- Serde-compatible data structures for plugin communication

## Usage

For plugin development (guest):
```toml
[dependencies]
touring-plugin-interface = { version = "0.1", features = ["guest"] }
```

For host applications:
```toml
[dependencies]
touring-plugin-interface = { version = "0.1", features = ["host"] }
```

## License

Licensed under either of

 * Apache License, Version 2.0
   ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license
   ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.
