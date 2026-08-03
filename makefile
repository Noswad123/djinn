APP_NAME = djinn
BIN_DIR = ./bin
INSTALL_DIR ?= $(HOME)/.local/bin
BUDDY_ROOT = ./tools/buddy
BUDDY_PACKAGE = $(BUDDY_ROOT)/packages/opencode

.PHONY: build check fmt install install-djinn buddy-deps build-buddy install-buddy legacy-go-build

build:
	@echo "🔨 Building Rust $(APP_NAME)..."
	cargo build --workspace

check:
	cargo check --workspace

fmt:
	cargo fmt --all

install: install-djinn install-buddy

install-djinn: build
	@echo "📦 Installing to $(INSTALL_DIR)/$(APP_NAME)"
	@mkdir -p "$(INSTALL_DIR)"
	install -m 0755 "target/debug/$(APP_NAME)" "$(INSTALL_DIR)/$(APP_NAME)"
	@if command -v xattr >/dev/null 2>&1; then \
		xattr -d com.apple.quarantine "$(INSTALL_DIR)/$(APP_NAME)" 2>/dev/null || true; \
	fi
	@echo "✅ Installed. Run with: $(APP_NAME)"

buddy-deps:
	@if ! command -v bun >/dev/null 2>&1; then \
		echo "bun is required to install Buddy from $(BUDDY_ROOT)" >&2; \
		exit 1; \
	fi
	bun install --cwd "$(BUDDY_ROOT)"

build-buddy: buddy-deps
	@echo "🔨 Building Buddy from $(BUDDY_PACKAGE)..."
	bun run --cwd "$(BUDDY_PACKAGE)" build --skip-install --skip-embed-web-ui

install-buddy: build-buddy
	@echo "📦 Installing Buddy to $(INSTALL_DIR)/buddy"
	@mkdir -p "$(INSTALL_DIR)"
	@set -eu; \
	found=; \
	for candidate in "$(BUDDY_PACKAGE)"/dist/buddy-*/bin/buddy "$(BUDDY_ROOT)"/dist/buddy-*/bin/buddy; do \
		if [ -x "$$candidate" ]; then \
			install -m 0755 "$$candidate" "$(INSTALL_DIR)/buddy"; \
			found=1; \
			break; \
		fi; \
	done; \
	if [ "$$found" = "" ]; then \
		echo "Buddy build output not found under $(BUDDY_PACKAGE)/dist" >&2; \
		exit 1; \
	fi
	@if command -v xattr >/dev/null 2>&1; then \
		xattr -d com.apple.quarantine "$(INSTALL_DIR)/buddy" 2>/dev/null || true; \
	fi
	@echo "✅ Installed. Run with: buddy"

legacy-go-build:
	$(MAKE) -C legacy/go build
