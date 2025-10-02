# Plugin Interface Library Makefile
# Manages WIT files and generates bindings for both host and guest

WIT_DIR := $(CURDIR)/
SRC_DIR := $(CURDIR)/src
WORLD   := library

# Generated binding files
HOST_BINDINGS := $(SRC_DIR)/host.rs
GUEST_BINDINGS := $(SRC_DIR)/guest.rs
LIB_FILE := $(SRC_DIR)/lib.rs

# Upstream repos and refs (override with env to pin a commit/tag)
GIT ?= git
WASI_HTTP_REPO   ?= https://github.com/WebAssembly/wasi-http.git
WASI_HTTP_REF    ?= main

CACHE_DIR := $(WIT_DIR)/_cache
CACHE_WASI_HTTP := $(CACHE_DIR)/wasi-http

DEPS_ROOT := $(WIT_DIR)/deps
PKG_WASI_HTTP   := $(DEPS_ROOT)/wasi-http
PKG_WASI_IO     := $(DEPS_ROOT)/wasi-io
PKG_WASI_CLOCKS := $(DEPS_ROOT)/wasi-clocks
PKG_WASI_RANDOM := $(DEPS_ROOT)/wasi-random
PKG_WASI_CLI    := $(DEPS_ROOT)/wasi-cli
PKG_WASI_FILESYSTEM := $(DEPS_ROOT)/wasi-filesystem
PKG_WASI_SOCKETS   := $(DEPS_ROOT)/wasi-sockets

FETCH_DIRS := \
	$(PKG_WASI_HTTP) \
	$(PKG_WASI_IO) \
	$(PKG_WASI_CLOCKS) \
	$(PKG_WASI_RANDOM) \
	$(PKG_WASI_CLI) \
	$(PKG_WASI_FILESYSTEM) \
	$(PKG_WASI_SOCKETS)

.PHONY: all bindings host-bindings guest-bindings clean check install-deps fetch-wit-deps update-wit-deps validate help

all: check bindings

# Generate both host and guest bindings
bindings: fetch-wit-deps host-bindings guest-bindings
	@echo "All bindings generated successfully"

# Generate host bindings (for wasmtime runtime)
host-bindings: check $(HOST_BINDINGS)
	@echo "Host bindings generated at $(HOST_BINDINGS)"

# Generate guest bindings (for WASM plugins)
guest-bindings: check $(GUEST_BINDINGS)
	@echo "Guest bindings generated at $(GUEST_BINDINGS)"

$(HOST_BINDINGS): $(WIT_DIR)/world.wit
	@mkdir -p $(SRC_DIR)
	@echo "Generating host bindings..."
	@wit-bindgen rust --generate-all --world $(WORLD) --out-dir $(SRC_DIR) $(WIT_DIR)
	@mv $(SRC_DIR)/library.rs $(HOST_BINDINGS) 2>/dev/null || cp $(SRC_DIR)/library.rs $(HOST_BINDINGS)
	@command -v rustfmt >/dev/null 2>&1 && rustfmt $(HOST_BINDINGS) || true

$(GUEST_BINDINGS): $(WIT_DIR)/world.wit
	@mkdir -p $(SRC_DIR)
	@echo "Generating guest bindings..."
	@wit-bindgen rust --generate-all --world $(WORLD) --out-dir $(SRC_DIR) $(WIT_DIR)
	@cp $(SRC_DIR)/library.rs $(GUEST_BINDINGS)
	@command -v rustfmt >/dev/null 2>&1 && rustfmt $(GUEST_BINDINGS) || true

# Check dependencies
check:
	@command -v wit-bindgen >/dev/null 2>&1 || \
	(echo "wit-bindgen not found. Install with: cargo install wit-bindgen-cli" && exit 1)
	@command -v $(GIT) >/dev/null 2>&1 || \
	(echo "git not found. Please install git." && exit 1)

# Install required tools
install-deps:
	@echo "Installing wit-bindgen..."
	@cargo install wit-bindgen-cli
	@echo "Installing rustfmt..."
	@rustup component add rustfmt

# Clean generated files
clean:
	@rm -f $(HOST_BINDINGS) $(GUEST_BINDINGS)
	@echo "Cleaned generated bindings"

# Validate WIT files
validate: check
	@echo "Validating WIT files..."
	@wit-bindgen rust --world $(WORLD) --out-dir /tmp --dry-run $(WIT_DIR) >/dev/null
	@echo "WIT files are valid"

# Clone or update wasi-http into cache and checkout ref
$(CACHE_WASI_HTTP):
	@mkdir -p $(CACHE_DIR)
	@if [ ! -d "$(CACHE_WASI_HTTP)/.git" ]; then \
		$(GIT) clone --depth 1 --branch $(WASI_HTTP_REF) $(WASI_HTTP_REPO) $(CACHE_WASI_HTTP); \
	else \
		cd $(CACHE_WASI_HTTP) && $(GIT) fetch --depth 1 origin $(WASI_HTTP_REF) && $(GIT) checkout -q FETCH_HEAD; \
	fi

# Fetch WIT dependencies from wasi-http repo (includes transitive deps at matching versions)
fetch-wit-deps: $(CACHE_WASI_HTTP)
	@echo "Syncing WIT dependencies into $(DEPS_ROOT) ..."
	@rm -rf $(PKG_WASI_HTTP) $(PKG_WASI_IO) $(PKG_WASI_CLOCKS) $(PKG_WASI_RANDOM) $(PKG_WASI_CLI) $(PKG_WASI_FILESYSTEM) $(PKG_WASI_SOCKETS)
	@mkdir -p $(FETCH_DIRS)
	@# Copy top-level wasi-http package files
	@cp $(CACHE_WASI_HTTP)/wit/*.wit $(PKG_WASI_HTTP)/
	@# Copy transitive deps provided by wasi-http (ensures exact version match)
	@cp $(CACHE_WASI_HTTP)/wit/deps/io/*.wit         $(PKG_WASI_IO)/
	@cp $(CACHE_WASI_HTTP)/wit/deps/clocks/*.wit     $(PKG_WASI_CLOCKS)/
	@cp $(CACHE_WASI_HTTP)/wit/deps/random/*.wit     $(PKG_WASI_RANDOM)/ 2>/dev/null || true
	@cp $(CACHE_WASI_HTTP)/wit/deps/cli/*.wit        $(PKG_WASI_CLI)/    2>/dev/null || true
	@cp $(CACHE_WASI_HTTP)/wit/deps/filesystem/*.wit $(PKG_WASI_FILESYSTEM)/ 2>/dev/null || true
	@cp $(CACHE_WASI_HTTP)/wit/deps/sockets/*.wit    $(PKG_WASI_SOCKETS)/    2>/dev/null || true
	@echo "WIT dependencies synced."

# Force update of cached repo
update-wit-deps:
	@rm -rf $(CACHE_DIR)
	@$(MAKE) fetch-wit-deps

# Show help
help:
	@echo "Plugin Interface Library Makefile"
	@echo ""
	@echo "Targets:"
	@echo "  all           - Generate all bindings (default)"
	@echo "  bindings      - Generate both host and guest bindings"
	@echo "  host-bindings - Generate host bindings only"
	@echo "  guest-bindings- Generate guest bindings only"
	@echo "  validate      - Validate WIT files"
	@echo "  install-deps  - Install required tools"
	@echo "  fetch-wit-deps- Fetch WIT deps from wasi-http (with transitive deps)"
	@echo "  update-wit-deps- Force refresh cached WIT repo"
	@echo "  clean         - Remove generated bindings"
	@echo "  help          - Show this help"
