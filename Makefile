EXT_NAME    := pg_perishable
VERSION     := $(shell awk -F'"' '/^version/ {print $$2; exit}' Cargo.toml)
PG_CONFIG   ?= pg_config

PKGLIBDIR   := $(shell $(PG_CONFIG) --pkglibdir 2>/dev/null)
SHAREDIR    := $(shell $(PG_CONFIG) --sharedir 2>/dev/null)
EXTDIR      := $(SHAREDIR)/extension

UNAME_S     := $(shell uname -s)
ifeq ($(UNAME_S),Darwin)
    SOEXT   := dylib
else
    SOEXT   := so
endif

TARGET_SO   := target/release/lib$(EXT_NAME).$(SOEXT)
DIST_DIR    := dist
PKG_NAME    := $(EXT_NAME)-$(VERSION)-$(UNAME_S)-$(shell uname -m)

.PHONY: all check-tools check-pg-config build schema package install \
        uninstall clean distclean post-install-notes help

all: help

help:
	@echo "pg_perishable Makefile targets:"
	@echo "  make check-tools    - verify cargo, cargo-pgrx, pg_config are available"
	@echo "  make build          - compile the extension (release mode)"
	@echo "  make schema         - regenerate the .sql install script only"
	@echo "  make package        - build + bundle .so/.control/.sql into dist/*.tar.gz"
	@echo "  make install        - copy built files into this machine's Postgres dirs"
	@echo "  make uninstall      - remove pg_perishable files from this machine's Postgres dirs"
	@echo "  make clean          - remove build artifacts"
	@echo "  make distclean      - clean + remove dist/ packages"
	@echo "  make post-install-notes - print the remaining manual steps"

# ---------------------------------------------------------------------------
# Tooling checks — fail early with a clear message rather than a cryptic
# error partway through a build.
# ---------------------------------------------------------------------------
check-tools:
	@command -v cargo >/dev/null 2>&1 || { echo "ERROR: cargo not found. Install Rust: https://rustup.rs"; exit 1; }
	@command -v cargo-pgrx >/dev/null 2>&1 || { echo "ERROR: cargo-pgrx not found. Install: cargo install --locked cargo-pgrx"; exit 1; }
	@$(MAKE) check-pg-config

check-pg-config:
	@command -v $(PG_CONFIG) >/dev/null 2>&1 || { \
		echo "ERROR: pg_config not found (checked: $(PG_CONFIG))."; \
		echo "Set PG_CONFIG=/path/to/pg_config if you have multiple Postgres installs."; \
		exit 1; \
	}
	@echo "Using pg_config: $$(command -v $(PG_CONFIG))"
	@echo "  pkglibdir: $(PKGLIBDIR)"
	@echo "  sharedir:  $(SHAREDIR)"

# ---------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------
build: check-tools
	cargo pgrx package

# Regenerate only the .sql file, without a full package build — useful
# during development after adding/changing a #[pg_extern] function.
schema: check-tools
	cargo pgrx schema

# ---------------------------------------------------------------------------
# Package — bundles the build output into a distributable tarball,
# matching the layout the README's "download a release" instructions expect.
# ---------------------------------------------------------------------------
package: build
	@mkdir -p $(DIST_DIR)
	@BUILD_DIR=$$(find target -type d -name "$(EXT_NAME)-pg*" | head -n1); \
	if [ -z "$$BUILD_DIR" ]; then \
		echo "ERROR: could not locate cargo-pgrx package output under target/"; \
		exit 1; \
	fi; \
	echo "Packaging from $$BUILD_DIR"; \
	SO_FILE=$$(find "$$BUILD_DIR" -name "lib$(EXT_NAME).$(SOEXT)"); \
	CONTROL_FILE=$$(find "$$BUILD_DIR" -name "$(EXT_NAME).control"); \
	SQL_FILE=$$(find "$$BUILD_DIR" -name "$(EXT_NAME)--*.sql"); \
	STAGE=$$(mktemp -d); \
	cp "$$SO_FILE" "$$STAGE/$(EXT_NAME).$(SOEXT)"; \
	cp "$$CONTROL_FILE" "$$STAGE/"; \
	cp "$$SQL_FILE" "$$STAGE/"; \
	tar -czf "$(DIST_DIR)/$(PKG_NAME).tar.gz" -C "$$STAGE" .; \
	rm -rf "$$STAGE"; \
	echo "Built $(DIST_DIR)/$(PKG_NAME).tar.gz"

# ---------------------------------------------------------------------------
# Install — copies already-built files into this machine's live Postgres
# directories. Intended for local development/testing, not as the primary
# end-user install path (that's the downloaded-release flow in the README).
# Requires write access to $(PKGLIBDIR)/$(EXTDIR) — typically needs sudo.
# ---------------------------------------------------------------------------
install: check-pg-config build
	@test -n "$(PKGLIBDIR)" || { echo "ERROR: pkglibdir empty — check pg_config"; exit 1; }
	@test -n "$(EXTDIR)" || { echo "ERROR: sharedir empty — check pg_config"; exit 1; }
	@BUILD_DIR=$$(find target -type d -name "$(EXT_NAME)-pg*" | head -n1); \
	SO_FILE=$$(find "$$BUILD_DIR" -name "lib$(EXT_NAME).$(SOEXT)"); \
	CONTROL_FILE=$$(find "$$BUILD_DIR" -name "$(EXT_NAME).control"); \
	SQL_FILE=$$(find "$$BUILD_DIR" -name "$(EXT_NAME)--*.sql"); \
	install -d "$(EXTDIR)"; \
	install -m 755 "$$SO_FILE" "$(PKGLIBDIR)/$(EXT_NAME).$(SOEXT)"; \
	install -m 644 "$$CONTROL_FILE" "$(EXTDIR)/"; \
	install -m 644 "$$SQL_FILE" "$(EXTDIR)/"; \
	echo "Installed to $(PKGLIBDIR) and $(EXTDIR)"
	@$(MAKE) post-install-notes

uninstall: check-pg-config
	rm -f "$(PKGLIBDIR)/$(EXT_NAME).$(SOEXT)"
	rm -f "$(EXTDIR)/$(EXT_NAME).control"
	rm -f "$(EXTDIR)"/$(EXT_NAME)--*.sql
	@echo "Removed pg_perishable files from $(PKGLIBDIR) and $(EXTDIR)"
	@echo "Note: run 'DROP EXTENSION pg_perishable;' in psql first if it's still CREATEd,"
	@echo "and remove it from shared_preload_libraries before restarting Postgres."

# ---------------------------------------------------------------------------
# Cleanup
# ---------------------------------------------------------------------------
clean:
	cargo clean

distclean: clean
	rm -rf $(DIST_DIR)

# ---------------------------------------------------------------------------
# What's left for the human — deliberately not automated by this Makefile.
# ---------------------------------------------------------------------------
post-install-notes:
	@echo ""
	@echo "Files are installed. Remaining manual steps:"
	@echo ""
	@echo "  1. Add to postgresql.conf:"
	@echo "       shared_preload_libraries = '$(EXT_NAME)'"
	@echo ""
	@echo "  2. Restart Postgres; required: shared_preload_libraries is only"
	@echo "       read at process startup, not on reload)."
	@echo ""
	@echo "  3. Run in psql:"
	@echo "       CREATE EXTENSION $(EXT_NAME);"
	@echo ""
	@echo "  4. In case a datacenter gets bombed, whether or not you restart Postgres,"
	@echo "     this won't matter a bit :)"
	@echo ""
