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

# The protected-secrets probes (tests/protected/): a throwaway container from
# IMAGE, made-up secrets only, one line per probe, exit 1 if any result
# differs from EXPECT. RUNTIME is docker, podman or container.
#   make protected-probes RUNTIME=container IMAGE=hotline-computer:0.12.0-rc.3 EXPECT=leak
# SPLIT=1 (or MODE=split) creates it as the 0.50 desk does, else as 0.49 does;
# DELIVERY=readable|protected sends a v2 body (protected needs a probe build:
# --features probe-roots, test only).
#   make protected-probes RUNTIME=container IMAGE=<probe build> SPLIT=1 DELIVERY=protected EXPECT=blocked
.PHONY: protected-probes
protected-probes:
	@test -n "$(RUNTIME)" -a -n "$(IMAGE)" -a -n "$(EXPECT)" || { echo "usage: make protected-probes RUNTIME=docker|podman|container IMAGE=<ref> EXPECT=leak|blocked [SPLIT=1|MODE=split] [DELIVERY=v1|readable|protected] [ONLY=id,id] [EXCEPT=id,id] [PREFIX=hl-secrets-w5]"; exit 2; }
	python3 tests/protected/run.py --runtime $(RUNTIME) --image $(IMAGE) --expect $(EXPECT) \
	  $(if $(or $(SPLIT),$(filter split,$(MODE))),--split) $(if $(DELIVERY),--delivery $(DELIVERY)) $(if $(PREFIX),--prefix $(PREFIX)) \
	  $(if $(ONLY),--only $(ONLY)) $(if $(EXCEPT),--except $(EXCEPT))
