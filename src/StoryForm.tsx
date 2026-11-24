import { useState, type FormEvent } from "react";

interface Props {
  onCreate: (title: string, text: string) => Promise<void>;
}

export function StoryForm({ onCreate }: Props) {
  const [title, setTitle] = useState("");
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);

  async function submit(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    try {
      await onCreate(title, text);
      setTitle("");
      setText("");
    } finally {
      setBusy(false);
    }
  }

  return (
    <form className="story-form" onSubmit={submit}>
      <input
        placeholder="Название"
        value={title}
        onChange={(e) => setTitle(e.target.value)}
      />
      <textarea
        placeholder="Текст истории"
        rows={6}
        value={text}
        onChange={(e) => setText(e.target.value)}
      />
      <button disabled={busy || !title.trim() || !text.trim()}>Добавить</button>
    </form>
  );
}
