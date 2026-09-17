IMAGE ?= hotline-computer:next
NAME ?= hotline-computer-next
PORT ?= 8787

.PHONY: check image run stop contract logs

check:
	cargo fmt --all --check
	cargo clippy --all-targets -- -D warnings
	cargo test

image:
	docker build -t $(IMAGE) .

# A fresh token per run, kept in .token for `make contract`. The container
# runs with every capability dropped: nothing in it is root.
run: stop
	openssl rand -hex 24 > .token
	docker run -d --name $(NAME) \
	  --cap-drop=ALL --security-opt no-new-privileges \
	  --pids-limit 1024 --memory 4g --shm-size 1g \
	  -p 127.0.0.1:$(PORT):8787 \
	  -e HOTLINE_COMPUTER_TOKEN="$$(cat .token)" \
	  -e TZ="$$(cat /etc/timezone 2>/dev/null || echo UTC)" \
	  $(IMAGE)

stop:
	-docker rm -f $(NAME) >/dev/null 2>&1

# The contract test drives the running container through a real MCP client.
contract:
	HOTLINE_COMPUTER_URL=http://127.0.0.1:$(PORT) HOTLINE_COMPUTER_TOKEN="$$(cat .token)" \
	  cargo test --test contract -- --nocapture

logs:
	docker logs $(NAME)

# Builds and tests the runtime on a fresh desktop.
.PHONY: acceptance
acceptance:
	docker build --target checks -t hotline-computer:checks .
	docker build -t $(IMAGE) .
	tests/run-image-acceptance.sh $(IMAGE) hotline-computer:checks qa/latest
