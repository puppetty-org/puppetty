# VHS render environment for the README demo GIF.
# VHS hangs on Windows hosts, so we render inside the official Linux image:
#   docker build -f docs/vhs.Dockerfile -t puppetty-vhs .
#   docker run --rm -v "$PWD:/vhs" puppetty-vhs docs/demo.tape
FROM ghcr.io/charmbracelet/vhs:latest
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates curl \
  && curl -fsSL https://deb.nodesource.com/setup_22.x | bash - \
  && apt-get install -y --no-install-recommends nodejs \
  && npm install -g puppetty \
  && rm -rf /var/lib/apt/lists/*
