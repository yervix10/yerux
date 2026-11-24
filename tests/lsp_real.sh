#!/usr/bin/env bash
# Scripted LSP sessions over examples/real/* (see tests/lsp_session.py); --all renames every item and local.
cd "$(dirname "$0")/.." || exit 2
exec python3 tests/lsp_session.py "$@"
