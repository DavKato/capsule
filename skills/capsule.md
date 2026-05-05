---
name: capsule
description: Routes capsule-related tasks to the capsule CLI's self-describing surface.
---

When the user asks for capsule setup:
  Run `capsule templates list` to see options, then `capsule init --template <name>`.

When the user asks to customize, debug, or migrate an existing capsule setup:
  Run `capsule explain` to see the topic index, then load relevant topics with
  `capsule explain <topic> [<topic>…]` in a single call.
