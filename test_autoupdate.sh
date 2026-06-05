#!/bin/bash
# Test fluxmux auto-update in Docker with host network
set -e

echo "=== FluxMux Auto-Update Test ==="
echo "Current binary version: 0.9.17"
echo "Remote version file: $(cat /home/orobit/q-narwhalknight/dist-final/downloads/fluxmux.version)"
echo ""

# Clean up any previous test artifacts
rm -f /tmp/flux-update.ready /tmp/flux-gemma.response /tmp/flux-gemma.done /tmp/flux-webhooks.queue

# Run fluxmux in background for 5 seconds (enough for update check)
echo "Starting fluxmux in Docker (5s timeout)..."
timeout 8 docker run --rm --network host \
    -v /home/orobit/q-narwhalknight/dist-final/downloads/fluxmux:/fluxmux:ro \
    --entrypoint /fluxmux \
    rust:bookworm 2>/dev/null &
FPID=$!

# Wait for update check to fire
sleep 6

# Check results
echo ""
echo "=== Results ==="
if [ -f /tmp/flux-update.ready ]; then
    echo "✅ UPDATE FLAG: $(cat /tmp/flux-update.ready)"
else
    echo "❌ No update flag detected"
fi

if [ -f /tmp/fluxmux.new ]; then
    echo "✅ Downloaded binary: $(stat -c%s /tmp/fluxmux.new) bytes"
else
    echo "❌ No downloaded binary"
fi

# Show webhook events
if [ -f /tmp/flux-webhooks.queue ]; then
    echo "Webhook events:"
    cat /tmp/flux-webhooks.queue
fi

# Cleanup
kill $FPID 2>/dev/null || true
echo ""
echo "=== Test Complete ==="
