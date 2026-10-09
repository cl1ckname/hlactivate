PREFIX ?= $(HOME)/.local
BINDIR ?= $(PREFIX)/bin
BIN := hlactivate

.PHONY: all build install uninstall restart clean

all: build

build:
	cargo build --release

install: build
	install -Dm755 target/release/$(BIN) $(BINDIR)/$(BIN)

uninstall:
	rm -f $(BINDIR)/$(BIN)

restart:
	-pkill -x $(BIN)
	setsid -f $(BINDIR)/$(BIN) >/dev/null 2>&1

clean:
	cargo clean
