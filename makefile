APP_NAME = djinn
BIN_DIR = ./bin
INSTALL_DIR ?= $(HOME)/.local/bin
DJINN_UI_ROOT = ./clients/djinn-ui
DJINN_UI_PACKAGE = $(DJINN_UI_ROOT)/packages/opencode

.PHONY: build check fmt install install-djinn ui-deps build-ui install-ui legacy-go-build

build:
	@echo "🔨 Building Rust $(APP_NAME)..."
	cargo build --workspace

check:
	cargo check --workspace

fmt:
	cargo fmt --all

install: install-djinn install-ui

install-djinn: build
	@echo "📦 Installing to $(INSTALL_DIR)/$(APP_NAME)"
	@mkdir -p "$(INSTALL_DIR)"
	install -m 0755 "target/debug/$(APP_NAME)" "$(INSTALL_DIR)/$(APP_NAME)"
	@if command -v xattr >/dev/null 2>&1; then \
		xattr -d com.apple.quarantine "$(INSTALL_DIR)/$(APP_NAME)" 2>/dev/null || true; \
	fi
	@echo "✅ Installed. Run with: $(APP_NAME)"

ui-deps:
	@if ! command -v bun >/dev/null 2>&1; then \
		echo "bun is required to install the Djinn UI from $(DJINN_UI_ROOT)" >&2; \
		exit 1; \
	fi
	bun install --cwd "$(DJINN_UI_ROOT)"

build-ui: ui-deps
	@echo "🔨 Building Djinn UI from $(DJINN_UI_PACKAGE)..."
	bun run --cwd "$(DJINN_UI_PACKAGE)" build --skip-install --skip-embed-web-ui

install-ui: build-ui
	@echo "📦 Installing Djinn UI to $(INSTALL_DIR)/djinn-ui"
	@mkdir -p "$(INSTALL_DIR)"
	@set -eu; \
	found=; \
	for candidate in "$(DJINN_UI_PACKAGE)"/dist/djinn-ui-*/bin/djinn-ui "$(DJINN_UI_ROOT)"/dist/djinn-ui-*/bin/djinn-ui; do \
		if [ -x "$$candidate" ]; then \
			install -m 0755 "$$candidate" "$(INSTALL_DIR)/djinn-ui"; \
			found=1; \
			break; \
		fi; \
	done; \
	if [ "$$found" = "" ]; then \
		echo "Djinn UI build output not found under $(DJINN_UI_PACKAGE)/dist" >&2; \
		exit 1; \
	fi
	@if command -v xattr >/dev/null 2>&1; then \
		xattr -d com.apple.quarantine "$(INSTALL_DIR)/djinn-ui" 2>/dev/null || true; \
	fi
	@echo "✅ Installed. Run with: djinn"

legacy-go-build:
	$(MAKE) -C legacy/go build
