#!/bin/sh
printf '%s\t%s\t%s\n' "$*" "$INFRAHUB_ADDRESS" "$INFRAHUB_API_TOKEN" >> "$FAKE_UV_LOG"
if [ "$4" = schema ]; then
    printf '%s\n' '{"ok": true}'
else
    printf '%s\n' "$FAKE_UV_STDOUT"
fi
exit "${FAKE_UV_CODE:-0}"
