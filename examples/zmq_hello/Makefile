# zmq_hello — cross-language ZMQ demo: REQ/REP, PUSH/PULL, PUB/SUB
#
# Patterns:
#   REQ/REP   — server.mvl (simplified) + server_zmtp.mvl (ZMTP 3.x)
#   PUSH/PULL — server_pull.mvl (ZMTP 3.x PULL) + client_push.py (pyzmq PUSH)
#   PUB/SUB   — server_pub.py (pyzmq PUB) + client_sub.mvl (ZMTP 3.x SUB)
#
# Usage:
#   make test          — simplified REQ/REP: MVL server + raw Python client
#   make test-zmq      — ZMTP REQ/REP: MVL server + pyzmq client
#   make test-rust     — ZMTP REQ/REP: MVL server + Rust zmq client
#   make test-pushpull — ZMTP PUSH/PULL: MVL PULL server + pyzmq PUSH client
#   make test-pubsub   — ZMTP PUB/SUB: pyzmq PUB server + MVL SUB client
#   make test-all      — run all test modes
#   make check         — type-check all MVL sources
.PHONY: help guard-mvl guard-pyzmq check build test test-zmq test-rust \
        test-pushpull test-pubsub test-all run run-zmtp clean smoke test-solver

.DEFAULT_GOAL := help

MVL       := ../../target/debug/mvl
DIR       := $(dir $(abspath $(lastword $(MAKEFILE_LIST))))
REPO_ROOT := $(abspath $(DIR)../..)
PORT      := 5555
PORT_ZMTP := 5556
PORT_PULL := 5557
PORT_PUB  := 5558
TIMEOUT   := 10   # seconds to wait for server to bind

help: ## Show this help
	@grep -E '^[a-zA-Z0-9_-]+:.*?## .*$$' $(MAKEFILE_LIST) | \
		awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-14s\033[0m %s\n", $$1, $$2}'

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

guard-pyzmq: ## Ensure pyzmq is installed in the active venv
	@python3 -c "import zmq" 2>/dev/null || { \
		echo "  Installing pyzmq..."; \
		uv pip install pyzmq; \
	}

# Pre-build all MVL binaries so `mvl run` is instant (cached cargo build).
build: guard-mvl .mvl/pkg/zmq ## Build all MVL binaries
	@echo "  Building MVL binaries..."
	@$(MVL) build $(DIR)server.mvl > /dev/null 2>&1
	@$(MVL) build $(DIR)server_zmtp.mvl > /dev/null 2>&1
	@$(MVL) build $(DIR)server_pull.mvl > /dev/null 2>&1
	@$(MVL) build $(DIR)client_sub.mvl > /dev/null 2>&1

check: guard-mvl .mvl/pkg/zmq ## Type-check all MVL sources
	$(MVL) check $(DIR)server.mvl
	$(MVL) check $(DIR)server_zmtp.mvl
	$(MVL) check $(DIR)server_pull.mvl
	$(MVL) check $(DIR)client_sub.mvl

test-solver: guard-mvl .mvl/pkg/zmq ## Show solver statistics
	@pass=0; fail=0; \
	OK="\033[32m✓\033[0m"; FAIL="\033[31m✗\033[0m"; \
	for f in $(DIR)server.mvl $(DIR)server_zmtp.mvl $(DIR)server_pull.mvl $(DIR)client_sub.mvl; do \
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

test: build ## Simplified REQ/REP: MVL server + raw Python client
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

# ── ZMTP 3.x REQ/REP (port 5556) ────────────────────────────────────────────

test-zmq: build guard-pyzmq ## ZMTP REQ/REP: MVL server + pyzmq client
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

test-rust: build ## ZMTP REQ/REP: MVL server + Rust zmq client
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

# ── ZMTP 3.x PUSH/PULL (port 5557) ──────────────────────────────────────────

test-pushpull: build guard-pyzmq ## ZMTP PUSH/PULL: MVL PULL server + pyzmq PUSH client
	@echo ""
	@echo "  zmq_hello [push/pull]: starting PULL server on port $(PORT_PULL)..."
	@SERVER_PID=""; \
	cleanup() { \
		if [ -n "$$SERVER_PID" ]; then \
			kill "$$SERVER_PID" 2>/dev/null || true; \
			wait "$$SERVER_PID" 2>/dev/null || true; \
		fi; \
	}; \
	trap cleanup EXIT INT TERM; \
	$(MVL) run $(DIR)server_pull.mvl > /tmp/zmq_hello_pull.log 2>&1 & \
	SERVER_PID=$$!; \
	elapsed=0; \
	until nc -z 127.0.0.1 $(PORT_PULL) 2>/dev/null; do \
		sleep 1; elapsed=$$((elapsed + 1)); \
		if [ $$elapsed -ge $(TIMEOUT) ]; then \
			echo "  ERROR: server did not bind to port $(PORT_PULL) within $(TIMEOUT)s"; \
			cat /tmp/zmq_hello_pull.log; \
			exit 1; \
		fi; \
	done; \
	echo "  zmq_hello [push/pull]: server ready — running pyzmq PUSH client..."; \
	CLIENT_OUT=$$(python3 $(DIR)client_push.py 2>&1); RC=$$?; \
	if [ $$RC -ne 0 ]; then \
		echo "  ERROR: pyzmq PUSH client exited with code $$RC"; \
		echo "$$CLIENT_OUT"; \
		cat /tmp/zmq_hello_pull.log; \
		exit 1; \
	fi; \
	sleep 1; \
	SERVER_LOG=$$(cat /tmp/zmq_hello_pull.log 2>/dev/null); \
	RECVD=$$(echo "$$SERVER_LOG" | grep -c "^ recv:"); \
	if [ "$$RECVD" -ne 3 ]; then \
		echo "  ERROR: expected 3 received messages, got $$RECVD"; \
		echo "  Server log:"; echo "$$SERVER_LOG"; \
		echo "  Client output:"; echo "$$CLIENT_OUT"; \
		exit 1; \
	fi; \
	echo "$$CLIENT_OUT" | sed 's/^/    /'; \
	echo "  Server received:"; \
	echo "$$SERVER_LOG" | grep "^ recv:" | sed 's/^/    /'; \
	echo "  \033[32m✓  zmq_hello [push/pull] passed (3/3 messages)\033[0m"; \
	echo ""

# ── ZMTP 3.x PUB/SUB (port 5558) ────────────────────────────────────────────

test-pubsub: build guard-pyzmq ## ZMTP PUB/SUB: pyzmq PUB server + MVL SUB client
	@echo ""
	@echo "  zmq_hello [pub/sub]: starting pyzmq PUB server on port $(PORT_PUB)..."
	@PUB_PID=""; \
	cleanup() { \
		if [ -n "$$PUB_PID" ]; then \
			kill "$$PUB_PID" 2>/dev/null || true; \
			wait "$$PUB_PID" 2>/dev/null || true; \
		fi; \
	}; \
	trap cleanup EXIT INT TERM; \
	python3 $(DIR)server_pub.py > /tmp/zmq_hello_pub.log 2>&1 & \
	PUB_PID=$$!; \
	elapsed=0; \
	until nc -z 127.0.0.1 $(PORT_PUB) 2>/dev/null; do \
		sleep 1; elapsed=$$((elapsed + 1)); \
		if [ $$elapsed -ge $(TIMEOUT) ]; then \
			echo "  ERROR: PUB server did not bind to port $(PORT_PUB) within $(TIMEOUT)s"; \
			cat /tmp/zmq_hello_pub.log; \
			exit 1; \
		fi; \
	done; \
	echo "  zmq_hello [pub/sub]: PUB server ready — running MVL SUB client..."; \
	CLIENT_OUT=$$(timeout 15 $(MVL) run $(DIR)client_sub.mvl 2>&1); RC=$$?; \
	RECVD=$$(echo "$$CLIENT_OUT" | grep -c "^ recv:"); \
	if [ "$$RECVD" -ne 3 ]; then \
		echo "  ERROR: expected 3 received messages, got $$RECVD"; \
		echo "  Client output:"; echo "$$CLIENT_OUT"; \
		echo "  PUB server log:"; cat /tmp/zmq_hello_pub.log; \
		exit 1; \
	fi; \
	echo "$$CLIENT_OUT" | grep "^ recv:" | sed 's/^/    /'; \
	echo "  \033[32m✓  zmq_hello [pub/sub] passed (3/3 messages)\033[0m"; \
	echo ""

# ── Run all ──────────────────────────────────────────────────────────────────

test-all: test test-zmq test-rust test-pushpull test-pubsub ## Run all test modes

smoke: build ## Smoke-test: verify all MVL binaries compile
	@echo "  \033[32m✓  all MVL binaries built successfully\033[0m"

run: build ## Start the simplified REP server (Ctrl-C to stop)
	$(MVL) run $(DIR)server.mvl

run-zmtp: build ## Start the ZMTP REP server (Ctrl-C to stop)
	$(MVL) run $(DIR)server_zmtp.mvl

run-pull: build ## Start the ZMTP PULL server (Ctrl-C to stop)
	$(MVL) run $(DIR)server_pull.mvl

clean: ## Remove temporary log files and .mvl symlinks
	rm -f /tmp/zmq_hello_server.log /tmp/zmq_hello_zmtp.log
	rm -f /tmp/zmq_hello_pull.log /tmp/zmq_hello_pub.log
	rm -rf .mvl
