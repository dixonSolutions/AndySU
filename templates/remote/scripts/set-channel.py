#!/usr/bin/env python3
"""set-channel.py channels.toml <channel> <release> [--rollout N]: edit one channel, keep comments."""
import re
import sys

path, channel, release = sys.argv[1:4]
rollout = sys.argv[sys.argv.index("--rollout") + 1] if "--rollout" in sys.argv else None
text = open(path).read()
head = f"[channels.{channel}]"
if head not in text:
    text = text.rstrip("\n") + f"\n\n{head}\n"
start = text.index(head) + len(head)
nxt = re.search(r"^\[", text[start:], re.M)
end = start + nxt.start() if nxt else len(text)
body = text[start:end]
body = re.sub(r"^#?\s*release\s*=.*\n?", "", body, flags=re.M)
if rollout is not None:
    body = re.sub(r"^#?\s*rollout\s*=.*\n?", "", body, flags=re.M)
lines = f"\nrelease = \"{release}\"\n" + (f"rollout = {rollout}\n" if rollout is not None else "")
text = text[:start] + lines + body.lstrip("\n") + ("\n" if nxt and not body.endswith("\n\n") else "") + text[end:]
open(path, "w").write(re.sub(r"\n{3,}", "\n\n", text))
