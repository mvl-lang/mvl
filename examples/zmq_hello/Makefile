# zmq_hello — cross-language REQ/REP demo: MVL server + Python/Rust clients
#
# Two server modes:
#   server.mvl      — simplified ZMTP (4-byte length prefix, one-conn-per-msg)
#   server_zmtp.mvl — real ZMTP 3.x (interop with pyzmq, zmq.rs, cppzmq)
#
# Usage:
#   make test       — simplified mode: MVL server + raw Python client
#   make test-zmq   — ZMTP mode: MVL server + pyzmq client (requires pyzmq)
#   make test-rust  — ZMTP mode: MVL server + Rust zmq client (requires cargo)
#   make test-all   — run all three test modes
#   make check      — type-check both servers
.PHONY: help guard-mvl check test test-zmq test-rust test-all run run-zmtp clean

.DEFAULT_GOAL := help

MVL       := ../../target/debug/mvl
DIR       := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))
REPO_ROOT := $(abspath $(DIR)../..)
PORT      := 5555
PORT_ZMTP := 5556
TIMEOUT   := 10   # seconds to wait for server to bind

help: ## Show this help
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-12s\033[0m %s\n", $$1, $$2}'

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

check: guard-mvl .mvl/pkg/zmq ## Type-check both servers
	$(MVL) check $(DIR)server.mvl
	$(MVL) check $(DIR)server_zmtp.mvl

test-solver: guard-mvl .mvl/pkg/zmq ## Show solver statistics
	@pass=0; fail=0; \
	OK="\033[32m✓\033[0m"; FAIL="\033[31m✗\033[0m"; \
	for f in $(DIR)server.mvl $(DIR)server_zmtp.mvl; do \
		name=$$(basename "$$f"); \
		out=$$($(MVL) check "$$f" 2>&1); rc=$$?; \
		if [ $$rc -eq 0 ]; then \
			printf "  $$OK  %s\n" "$$name"; pass=$$((pass + 1)); \
		else \
			printf "  $$FAIL  %s\n" "$$name"; printf "%s\n" "$$out" | sed 's/^/         /'; fail=$$((fail + 1)); \
		fi; \
	done; \
	echo ""; \
	if [ $$fail -eq 0 ]; then \
		printf "  \033[32m✓  $$pass passed, 0 failed\033[0m\n\n"; \
	else \
		printf "  \033[31m✗  $$pass passed, $$fail failed\033[0m\n\n"; exit 1; \
	fi

# ── Simplified mode (original, port 5555) ────────────────────────────────────

test: guard-mvl .mvl/pkg/zmq ## Simplified mode: MVL server + raw Python client
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

# ── ZMTP 3.x mode (port 5556) ────────────────────────────────────────────────

guard-pyzmq: ## Ensure pyzmq is installed in the active venv
	@python3 -c "import zmq" 2>/dev/null || { \
		echo "  Installing pyzmq..."; \
		uv pip install pyzmq; \
	}

test-zmq: guard-mvl guard-pyzmq .mvl/pkg/zmq ## ZMTP mode: MVL server + pyzmq client
	@echo ""
	@echo "  zmq_hello [zmtp]: starting ZMTP server on port $(PORT_ZMTP)..."
	@SERVER_PID=""; \
	cleanup() { \
		if [ -n "$$SERVER_PID" ]; then \
			kill "$$SERVER_PID" 2>/dev/null || true; \
			wait "$$SERVER_PID" 2>/dev/null || true; \
		fi; \
	}; \
	trap cleanup EXIT INT TERM; \
	$(MVL) run $(DIR)server_zmtp.mvl > /tmp/zmq_hello_zmtp.log 2>&1 & \
	SERVER_PID=$$!; \
	elapsed=0; \
	until nc -z 127.0.0.1 $(PORT_ZMTP) 2>/dev/null; do \
		sleep 1; elapsed=$$((elapsed + 1)); \
		if [ $$elapsed -ge $(TIMEOUT) ]; then \
			echo "  ERROR: server did not bind to port $(PORT_ZMTP) within $(TIMEOUT)s"; \
			cat /tmp/zmq_hello_zmtp.log; \
			exit 1; \
		fi; \
	done; \
	echo "  zmq_hello [zmtp]: server ready — running pyzmq client..."; \
	CLIENT_OUT=$$(python3 $(DIR)client_zmq.py 2>&1); RC=$$?; \
	if [ $$RC -ne 0 ]; then \
		echo "  ERROR: pyzmq client exited with code $$RC"; \
		echo "$$CLIENT_OUT"; \
		cat /tmp/zmq_hello_zmtp.log; \
		exit 1; \
	fi; \
	REPLIES=$$(echo "$$CLIENT_OUT" | grep -c "^ reply:"); \
	if [ "$$REPLIES" -ne 3 ]; then \
		echo "  ERROR: expected 3 replies, got $$REPLIES"; \
		echo "$$CLIENT_OUT"; \
		exit 1; \
	fi; \
	echo "$$CLIENT_OUT" | sed 's/^/    /'; \
	echo "  \033[32m✓  zmq_hello [zmtp] passed (3/3 pyzmq replies)\033[0m"; \
	echo ""

test-rust: guard-mvl .mvl/pkg/zmq ## ZMTP mode: MVL server + Rust zmq client
	@echo ""
	@echo "  zmq_hello [rust]: starting ZMTP server on port $(PORT_ZMTP)..."
	@SERVER_PID=""; \
	cleanup() { \
		if [ -n "$$SERVER_PID" ]; then \
			kill "$$SERVER_PID" 2>/dev/null || true; \
			wait "$$SERVER_PID" 2>/dev/null || true; \
		fi; \
	}; \
	trap cleanup EXIT INT TERM; \
	$(MVL) run $(DIR)server_zmtp.mvl > /tmp/zmq_hello_zmtp.log 2>&1 & \
	SERVER_PID=$$!; \
	elapsed=0; \
	until nc -z 127.0.0.1 $(PORT_ZMTP) 2>/dev/null; do \
		sleep 1; elapsed=$$((elapsed + 1)); \
		if [ $$elapsed -ge $(TIMEOUT) ]; then \
			echo "  ERROR: server did not bind to port $(PORT_ZMTP) within $(TIMEOUT)s"; \
			cat /tmp/zmq_hello_zmtp.log; \
			exit 1; \
		fi; \
	done; \
	echo "  zmq_hello [rust]: server ready — building and running Rust client..."; \
	CLIENT_OUT=$$(cd $(DIR)client_rust && cargo run 2>&1); RC=$$?; \
	if [ $$RC -ne 0 ]; then \
		echo "  ERROR: Rust client exited with code $$RC"; \
		echo "$$CLIENT_OUT"; \
		cat /tmp/zmq_hello_zmtp.log; \
		exit 1; \
	fi; \
	REPLIES=$$(echo "$$CLIENT_OUT" | grep -c "^ reply:"); \
	if [ "$$REPLIES" -ne 3 ]; then \
		echo "  ERROR: expected 3 replies, got $$REPLIES"; \
		echo "$$CLIENT_OUT"; \
		exit 1; \
	fi; \
	echo "$$CLIENT_OUT" | sed 's/^/    /'; \
	echo "  \033[32m✓  zmq_hello [rust] passed (3/3 Rust ZMQ replies)\033[0m"; \
	echo ""

test-all: test test-zmq test-rust ## Run all test modes

smoke: guard-mvl .mvl/pkg/zmq ## Smoke-test: build binary (proves compilation without a terminal)
	$(MVL) build $(DIR)server.mvl
	$(MVL) build $(DIR)server_zmtp.mvl

run: guard-mvl .mvl/pkg/zmq ## Start the simplified REP server (Ctrl-C to stop)
	$(MVL) run $(DIR)server.mvl

run-zmtp: guard-mvl .mvl/pkg/zmq ## Start the ZMTP REP server (Ctrl-C to stop)
	$(MVL) run $(DIR)server_zmtp.mvl

clean: ## Remove temporary log files and .mvl symlinks
	rm -f /tmp/zmq_hello_server.log /tmp/zmq_hello_zmtp.log
	rm -rf .mvl
