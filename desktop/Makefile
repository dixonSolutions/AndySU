# AndySU Linux desktop host and sample payload.
#   make            build the host (links the core statically)
#   make test       run the live-swap/rollback integration test
ROOT     := $(abspath ..)
OUT      := $(ROOT)/target/desktop
CORE_LIB := $(ROOT)/target/release/libandysu_core.a
CFLAGS   ?= -O2 -g
CFLAGS   += -DANDYSU_TRANSPORT -Wall -Wextra -I$(ROOT)/core/include -Iinclude

all: $(OUT)/andysu-desktop-host

$(CORE_LIB): $(wildcard $(ROOT)/core/src/*.rs) $(ROOT)/core/Cargo.toml
	cd $(ROOT) && cargo build --release -p andysu-core --features ffi,transport

$(OUT)/andysu-desktop-host: host/main.c include/andysu_payload.h $(ROOT)/core/include/andysu.h $(CORE_LIB)
	@mkdir -p $(OUT)
	$(CC) $(CFLAGS) -o $@ host/main.c $(CORE_LIB) -lpthread -ldl -lm

test: all
	cd $(ROOT) && cargo build -p andysu-cli
	python3 tests/test_desktop.py

clean:
	rm -rf $(OUT)

.PHONY: all test clean
