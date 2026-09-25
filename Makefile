# pyxray — build, install and look at things.
#
# Everything lands under ./target, ./build and ./python; nothing is written
# to $HOME.

CARGO ?= cargo
PYTHON ?= python3
WIDTH ?= 112
EXAMPLE ?= examples/analysis.py

# The built extension's name differs per platform; maturin knows, so
# `install-ext` uses it. `install-ext-copy` is the no-maturin fallback.
UNAME_S := $(shell uname -s 2>/dev/null || echo Windows)
ifeq ($(findstring MINGW,$(UNAME_S))$(findstring MSYS,$(UNAME_S))$(findstring Windows,$(UNAME_S)),)
  ifeq ($(UNAME_S),Darwin)
    EXT_SRC := target/release/lib_pyxray.dylib
  else
    EXT_SRC := target/release/lib_pyxray.so
  endif
  EXT_DST := python/pyxray_agent/_pyxray.abi3.so
  PYX := target/release/pyx
else
  EXT_SRC := target/release/_pyxray.dll
  EXT_DST := python/pyxray_agent/_pyxray.pyd
  PYX := target/release/pyx.exe
endif

.PHONY: help build release test test-rust test-python check fmt fmt-check clippy \
        shellcheck install-ext install-ext-copy wheel sheet demo lookdev audit clean

help:
	@printf 'targets:\n'
	@printf '  build          debug build of the workspace\n'
	@printf '  release        optimised build (this is the one you want)\n'
	@printf '  test           cargo tests, then the Python suite\n'
	@printf '  check          fmt-check + clippy + shellcheck + test + audit\n'
	@printf '  install-ext    build the Python extension in place (maturin develop)\n'
	@printf '  install-ext-copy  the same by copying the built library (no maturin)\n'
	@printf '  wheel          build a wheel into dist/\n'
	@printf '  sheet          render every theme x layout to build/contact-sheet.html\n'
	@printf '  demo           render every theme to build/gallery/\n'
	@printf '  lookdev        the full lookdev page in build/lookdev/\n'
	@printf '  audit          contrast check of every colour theme (exit 1 on failure)\n'
	@printf '  clean          remove target/, build/ and the installed extension\n'

build:
	$(CARGO) build

release:
	$(CARGO) build --release
	@printf '\nbinary: $(PYX) (%s)\n' "$$(du -h $(PYX) | cut -f1)"

# The Python tests need an engine: the extension if `install-ext` has run,
# else the debug binary that `cargo test` just built.
test: test-rust test-python

test-rust:
	$(CARGO) test --workspace

test-python:
	PYXRAY_BIN=$${PYXRAY_BIN:-target/debug/pyx} $(PYTHON) -m pytest python/tests -q

check: fmt-check clippy shellcheck test audit

fmt:
	$(CARGO) fmt --all

fmt-check:
	$(CARGO) fmt --all -- --check

clippy:
	$(CARGO) clippy --workspace --all-targets -- -D warnings

shellcheck:
	shellcheck -s sh integrations/shim/python3 integrations/shim/install.sh \
	  integrations/pyxray-hook integrations/claude-code/hooks/pyxray-hook \
	  integrations/claude-code/install.sh integrations/hermes/install.sh \
	  integrations/opencode/install.sh integrations/tmux/pyxray-pane.sh

install-ext:
	maturin develop --release
	@printf 'installed the extension into the active Python\n'

install-ext-copy: release
	cp $(EXT_SRC) $(EXT_DST)
	@printf 'installed $(EXT_DST)\n'

wheel:
	maturin build --release --out dist

sheet: build
	@mkdir -p build
	./target/debug/pyx --contact-sheet build/contact-sheet.html --width $(WIDTH) $(EXAMPLE)

demo: build
	@mkdir -p build/gallery
	@for t in blueprint neon paper carbon amber ansi mono; do \
	  ./target/debug/pyx -t $$t -l dashboard -f svg -w $(WIDTH) -o build/gallery/$$t.svg $(EXAMPLE); \
	  ./target/debug/pyx -t $$t -l dashboard -f html -w $(WIDTH) -o build/gallery/$$t.html $(EXAMPLE); \
	done
	@printf 'wrote build/gallery/\n'

lookdev: release
	$(PYTHON) tools/render_fragments.py
	$(PYTHON) tools/build_lookdev.py build/lookdev
	@printf 'wrote build/lookdev/contact-sheet.html\n'

audit: build
	@PYXRAY_BIN=target/debug/pyx $(PYTHON) tools/contrast_audit.py

clean:
	$(CARGO) clean
	rm -rf build dist python/pyxray_agent/_pyxray*.so python/pyxray_agent/_pyxray*.pyd python/pyxray_agent/bin
