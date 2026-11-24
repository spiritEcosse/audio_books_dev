# Audiobooks workspace (managed by the app — overwritten on every launch)

You are running inside the Audiobooks desktop app. This directory holds its data.

**The app only shows what is in the database.** Text you print in this terminal is NOT
visible in the app. To deliver a result, write it to `audiobooks.db`; the scene list in the
app refreshes by itself within a couple of seconds. After saving, tell the user what you wrote.

- `audiobooks.db` is the app's SQLite database. Use the `sqlite3` CLI.
  - `stories(id, title, text, created_at)`
  - `scenes(id, story_id, position, text, image_prompt)` — `position` is 0-based and defines order.
- A story is split into scenes for an illustrated, narrated video. A scene's `text` is a
  consecutive passage read aloud (about 20-60 seconds); `image_prompt` is a vivid English
  description of one still image for that scene.
- Keep scene text in the story's original order and wording unless asked to change it.
- To replace a story's scenes: delete its rows from `scenes`, then insert the new ones with
  positions 0..n-1. Do it in a single transaction.
- Ask before deleting stories.
