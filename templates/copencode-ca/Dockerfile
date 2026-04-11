# Minimal castor image for end-to-end testing of `castors exec`.
#
# Why this exists:
#   `castors add` runs the image with `docker run -d`. Stock images like
#   `debian` or `alpine` use an interactive shell as their entrypoint, which
#   exits immediately under `-d` (no TTY attached). The container then sits in
#   `Exited` state and `castors exec` on a freshly added castor would have to
#   restart it every time. This image gives PID 1 a long-running placeholder
#   so the container stays in `Running` and `exec` is a single hop.
#
# Build:
#   docker build -t castors-dummy-ca:latest templates/dummy-castor-ca
#
# Use:
#   castors add castors-dummy-ca:latest . demo
#   castors exec demo            # drops into /bin/sh inside the container
#   exit                         # returns to host; container stays running
#   castors rm demo

FROM debian:trixie-slim

RUN apt-get update && apt-get install -y iputils-ping curl
RUN curl -fsSL https://opencode.ai/install | bash

# Enabling TUI
RUN mkdir -p /root/.cache/opencode/tmp
ENV BUN_TMPDIR=/root/.cache/opencode/tmp

COPY opencode.json /root/.config/opencode/opencode.json

# Install the mitmproxy CA certificate.
# Grab from docker cp castors-infra-mitm:/home/mitmproxy/.mitmproxy/mitmproxy-ca-cert.pem ./ca-cert.pem
COPY ca-cert.pem /usr/local/share/ca-certificates/castors-mitm.crt
RUN update-ca-certificates

# `sleep infinity` is a tiny PID 1 that ignores stdin and never exits on its
# own. The image's entrypoint is irrelevant once `castors exec` is wired in.
CMD ["sleep", "infinity"]
