APP_NAME = djinn
BIN_DIR = ./bin
INSTALL_DIR ?= $(HOME)/.local/libexec/arcana-dev/djinn/bin
DJINN_UI_ROOT = ./clients/djinn-ui
DJINN_UI_PACKAGE = $(DJINN_UI_ROOT)/packages/opencode
DJINN_UI_OUT ?= $(abspath .build/djinn-ui)

.PHONY: build check fmt install install-djinn ui-deps build-ui install-ui legacy-go-build

build:
	@echo "🔨 Building Rust $(APP_NAME)..."
	cargo build --workspace

check:
	cargo check --workspace

fmt:
	cargo fmt --all

install: build build-ui
	bun scripts/install.ts bundle "target/debug/$(APP_NAME)" "$(DJINN_UI_OUT)" "$(INSTALL_DIR)"

install-djinn: install

ui-deps:
	@if ! command -v bun >/dev/null 2>&1; then \
		echo "bun is required to install the Djinn UI from $(DJINN_UI_ROOT)" >&2; \
		exit 1; \
	fi
	bun install --frozen-lockfile --cwd "$(DJINN_UI_ROOT)"

build-ui: ui-deps
	@echo "🔨 Building Djinn UI from $(DJINN_UI_PACKAGE)..."
	bun scripts/install.ts build-ui "$(DJINN_UI_PACKAGE)" "$(DJINN_UI_OUT)"

install-ui: build-ui
	bun scripts/install.ts ui "$(DJINN_UI_OUT)" "$(INSTALL_DIR)"

legacy-go-build:
	$(MAKE) -C legacy/go build
