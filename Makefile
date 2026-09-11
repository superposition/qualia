QUALIA_SHM_NAME ?= /qualia_body
QUALIA_WEB_DIR ?= $(CURDIR)/web/public
NVCC           ?= /usr/local/cuda/bin/nvcc

# Orin Nano (sm_87) is the deployment target; the 4090 (sm_89) is where kernels
# are developed and verified. Both fatbins are produced by one build.
CUDAARCHS ?= 87-real;89-real

.PHONY: help agent agent-debug test-agent run-agent \
        cuda-service cuda-service-debug test-cuda-service run-cuda-service \
        cuda-fatbin cuda-fatbin-87 world-export compute-capabilities

help:
	@printf '%s\n' \
	  'make agent               Build qualia-agent in release mode' \
	  'make agent-debug         Build qualia-agent in debug mode' \
	  'make test-agent          Run qualia-agent crate tests' \
	  'make run-agent           Run qualia-agent with the local web dir' \
	  'make cuda-service        Build qualia-cuda-service in release mode' \
	  'make cuda-fatbin         Build the CUDA fatbins for sm_87 and sm_89' \
	  'make cuda-fatbin-87      Build the Orin Nano fatbin (sm_87) only' \
	  'make world-export        Curl /world/export from the local agent' \
	  'make compute-capabilities Curl /compute/capabilities from the local agent'

agent:
	cargo build --release --bin qualia-agent

agent-debug:
	cargo build --bin qualia-agent

test-agent:
	cargo test -p qualia-agent

cuda-service:
	cargo build --release --bin qualia-cuda-service

cuda-service-debug:
	cargo build --bin qualia-cuda-service

test-cuda-service:
	cargo test -p qualia-cuda-service

run-cuda-service:
	cargo run --release --bin qualia-cuda-service

# The 4090 is the development target and QUALIA_CUDA_SM selects the device at
# runtime, so one fatbin carrying both architectures is what ships.
cuda-fatbin:
	CUDAARCHS='$(CUDAARCHS)' NVCC=$(NVCC) cargo build -p qualia-cuda --features cuda --release

cuda-fatbin-87:
	CUDAARCHS='87-real' NVCC=$(NVCC) cargo build -p qualia-cuda --features cuda --release

run-agent:
	QUALIA_SHM_NAME=$(QUALIA_SHM_NAME) QUALIA_WEB_DIR=$(QUALIA_WEB_DIR) cargo run --release --bin qualia-agent

world-export:
	curl -ks https://127.0.0.1:8080/world/export

compute-capabilities:
	curl -ks https://127.0.0.1:8080/compute/capabilities
