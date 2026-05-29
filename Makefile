# Concerto top-level Makefile.
#
# Thin convenience wrapper around the per-platform install scripts. The
# real logic lives in scripts/install-macos.sh and scripts/uninstall-macos.sh
# (and, eventually, scripts/install-linux.sh for V1.0).

UNAME_S := $(shell uname -s)

.PHONY: install uninstall install-macos uninstall-macos help

help:
	@echo "Concerto install targets:"
	@echo "  make install         Install for the current platform ($(UNAME_S))"
	@echo "  make uninstall       Uninstall from the current platform"
	@echo "  make install-macos   Force the macOS LaunchAgent install"
	@echo "  make uninstall-macos Force the macOS LaunchAgent uninstall"
	@echo ""
	@echo "Linux systemd / Windows Service Manager support lands in V1.0."

install:
ifeq ($(UNAME_S),Darwin)
	@$(MAKE) install-macos
else
	@echo "make install: unsupported platform ($(UNAME_S)); Linux/Windows land in V1.0" >&2
	@exit 1
endif

uninstall:
ifeq ($(UNAME_S),Darwin)
	@$(MAKE) uninstall-macos
else
	@echo "make uninstall: unsupported platform ($(UNAME_S)); Linux/Windows land in V1.0" >&2
	@exit 1
endif

install-macos:
	@./scripts/install-macos.sh

uninstall-macos:
	@./scripts/uninstall-macos.sh
