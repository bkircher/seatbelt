.PHONY: all test build install clean uninstall

BINARY := target/release/seatbelt
INSTALL_BIN := $(HOME)/bin
INSTALL_CONFIG := $(HOME)/.config/seatbelt

all: test build install

test:
	cargo test
	cargo lint

build:
	cargo build --release

install: build
	@if [ -d "$(INSTALL_BIN)" ]; then \
		install -m 0755 "$(BINARY)" "$(INSTALL_BIN)/seatbelt"; \
		echo "Installed $(INSTALL_BIN)/seatbelt"; \
	fi
	mkdir -p "$(INSTALL_CONFIG)/profiles"
	cp -R configs/. "$(INSTALL_CONFIG)/"
	cp -R profiles/. "$(INSTALL_CONFIG)/profiles/"

clean:
	cargo clean

uninstall:
	@if [ -e "$(INSTALL_BIN)/seatbelt" ]; then \
		rm "$(INSTALL_BIN)/seatbelt"; \
	fi
	rm -rf "$(INSTALL_CONFIG)"
