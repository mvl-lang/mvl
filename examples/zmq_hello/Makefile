# zmq_hello — cross-language REQ/REP demo: MVL server + Python client
#
# Demonstrates pkg.zmq's simplified ZMTP framing over a raw TCP socket.
# No pyzmq or libzmq needed — protocol is a 4-byte big-endian length prefix.
#
# Usage:
#   make test   — start server, run client, assert all 3 replies, stop server
#   make run    — start server interactively (Ctrl-C to stop)
#   make check  — type-check server.mvl
.PHONY: help guard-mvl check test run clean

.DEFAULT_GOAL := help

MVL       := ../../target/debug/mvl
DIR       := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))
REPO_ROOT := $(abspath $(DIR)../..)
PORT      := 5555
TIMEOUT   := 10   # seconds to wait for server to bind

help: ## Show this help
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-10s\033[0m %s\n", $$1, $$2}'

guard-mvl: ## Validate MVL binary is present
	@$(MVL) --version > /dev/null 2>&1 || { \
		echo ""; \
		echo "  ERROR: MVL compiler not found at: $(MVL)"; \
		echo "  Run 'make build' from the repo root first."; \
		echo ""; \
		exit 1; \
	}

# Ensure pkg.zmq symlink exists for local package resolution.
.mvl/pkg/zmq:
	@mkdir -p .mvl/pkg
	@ln -sfn $(REPO_ROOT)/pkg/zmq .mvl/pkg/zmq

check: guard-mvl .mvl/pkg/zmq ## Type-check server.mvl
	$(MVL) check $(DIR)server.mvl

test: guard-mvl .mvl/pkg/zmq ## Start server, run Python client, assert 3 echo replies, stop server
	@echo ""
	@echo "  zmq_hello: starting REP server on port $(PORT)..."
	@SERVER_PID=""; \
	cleanup() { \
		if [ -n "$$SERVER_PID" ]; then \
			kill "$$SERVER_PID" 2>/dev/null || true; \
			wait "$$SERVER_PID" 2>/dev/null || true; \
		fi; \
	}; \
	trap cleanup EXIT INT TERM; \
	$(MVL) run $(DIR)server.mvl > /tmp/zmq_hello_server.log 2>&1 & \
	SERVER_PID=$$!; \
	elapsed=0; \
	until nc -z 127.0.0.1 $(PORT) 2>/dev/null; do \
		sleep 1; elapsed=$$((elapsed + 1)); \
		if [ $$elapsed -ge $(TIMEOUT) ]; then \
			echo "  ERROR: server did not bind to port $(PORT) within $(TIMEOUT)s"; \
			cat /tmp/zmq_hello_server.log; \
			exit 1; \
		fi; \
	done; \
	echo "  zmq_hello: server ready — running client..."; \
	CLIENT_OUT=$$(python3 $(DIR)client.py 2>&1); RC=$$?; \
	if [ $$RC -ne 0 ]; then \
		echo "  ERROR: client exited with code $$RC"; \
		echo "$$CLIENT_OUT"; \
		exit 1; \
	fi; \
	REPLIES=$$(echo "$$CLIENT_OUT" | grep -c "^ reply:"); \
	if [ "$$REPLIES" -ne 3 ]; then \
		echo "  ERROR: expected 3 replies, got $$REPLIES"; \
		echo "$$CLIENT_OUT"; \
		exit 1; \
	fi; \
	echo "$$CLIENT_OUT" | sed 's/^/    /'; \
	echo "  \033[32m✓  zmq_hello passed (3/3 replies)\033[0m"; \
	echo ""

run: guard-mvl .mvl/pkg/zmq ## Start the REP server interactively (Ctrl-C to stop)
	$(MVL) run $(DIR)server.mvl

clean: ## Remove temporary log files and .mvl symlinks
	rm -f /tmp/zmq_hello_server.log
	rm -rf .mvl
