# Character cards

A character card gives your companion a personality. It's a small TOML file whose fields are
composed into the LLM system prompt, and whose greeting opens the conversation.

## Fields

```toml
name           = "Aria"                                  # who they are
persona        = "A cheerful, curious AI companion."     # description / personality
speaking_style = "Warm, concise, and playful."           # how they talk
scenario       = "Hanging out with you at your desk."     # the situation/context
greeting       = "Hey! I'm Aria — what are we up to?"     # first message, shown on load
examples       = ""                                       # optional example dialogue
```

All fields are optional; empty ones are skipped when building the prompt. The composed system
prompt looks roughly like:

```
You are Aria.
A cheerful, curious AI companion.
Speaking style: Warm, concise, and playful.
Scenario: Hanging out with you at your desk.
```

## Editing

Open the **Character** tab, edit the fields, and they apply to the next message. The active card is
saved with your settings automatically.

## Saving & sharing

- **Export** writes the current card to `…/<config dir>/ferra-vrm/cards/<name>.toml`.
- **Load** — drag a `.toml` card onto the window. It replaces the active card, clears the chat, and
  re-greets with the new character's greeting.

Because cards are plain TOML, you can share them, version them, or hand-write them in any editor.

## Tips

- Keep `speaking_style` concrete ("short replies", "lots of emoji", "deadpan") — models follow
  style cues well.
- Use `examples` for a couple of sample exchanges if you want to lock in a voice; format them as
  simple `User:` / `Assistant:` lines.
- The `greeting` is purely local (it seeds the first assistant turn) — it isn't sent to the model as
  something it "said" unless the conversation continues.
