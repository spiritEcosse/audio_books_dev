# Audiobooks workspace

You are running inside the Audiobooks desktop app (Tauri). This directory holds its data.

- `audiobooks.db` is the app's SQLite database.
  - `stories(id, title, text, created_at)`
  - `scenes(id, story_id, position, text, image_prompt)` — `position` is 0-based and defines order.
- Use the `sqlite3` CLI to read and edit it. The app refreshes its scene list automatically.
- A story is split into scenes for an illustrated, narrated video. A scene's `text` is a
  consecutive passage read aloud (about 20-60 seconds); `image_prompt` is a vivid English
  description of one still image for that scene.
- Keep scene text in the story's original order and wording unless asked to change it.
- Ask before deleting stories or scenes.
