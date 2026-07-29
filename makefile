APP_NAME = djinn
BIN_DIR = ./bin
INSTALL_DIR ?= $(HOME)/.local/bin

.PHONY: build check fmt install legacy-go-build

build:
	@echo "🔨 Building Rust $(APP_NAME)..."
	cargo build --workspace

check:
	cargo check --workspace

fmt:
	cargo fmt --all

install: build
	@echo "📦 Installing to $(INSTALL_DIR)/$(APP_NAME)"
	@mkdir -p "$(INSTALL_DIR)"
	install -m 0755 "target/debug/$(APP_NAME)" "$(INSTALL_DIR)/$(APP_NAME)"
	@if command -v xattr >/dev/null 2>&1; then \
		xattr -d com.apple.quarantine "$(INSTALL_DIR)/$(APP_NAME)" 2>/dev/null || true; \
	fi
	@echo "✅ Installed. Run with: $(APP_NAME)"

legacy-go-build:
	$(MAKE) -C legacy/go build
