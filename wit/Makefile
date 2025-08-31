# Generate Rust host (wasmtime) bindings from wit/source.wit

WIT_DIR := $(CURDIR)
OUT_DIR := $(CURDIR)/../src/
WORLD   := source

BINDINGS_RS := $(OUT_DIR)/source.rs

.PHONY: bindings clean check

bindings: check $(BINDINGS_RS)
	@echo "Bindings generated at $(BINDINGS_RS)"

$(BINDINGS_RS): $(WIT_DIR)/source.wit
	@mkdir -p $(OUT_DIR)
	@wit-bindgen rust --world $(WORLD) --out-dir $(OUT_DIR) $(WIT_DIR)
	@command -v rustfmt >/dev/null 2>&1 && rustfmt $(BINDINGS_RS) || true

check:
	@command -v wit-bindgen >/dev/null 2>&1 || \
	(echo "wit-bindgen not found. Install with: cargo install wit-bindgen-cli" && exit 1)

clean:
	@rm -rf $(OUT_DIR)
	@echo "Cleaned generated bindings"