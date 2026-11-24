import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import type { Message, ModelsInfo } from "./api";

interface Props {
  messages: readonly Message[];
  pendingText: string | null;
  busy: boolean;
  models: ModelsInfo | null;
  onModelChange: (id: string) => void;
  onSend: (text: string) => Promise<void>;
}

export function Chat({ messages, pendingText, busy, models, onModelChange, onSend }: Props) {
  const [draft, setDraft] = useState("");
  const endRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    endRef.current?.scrollIntoView({ block: "end" });
  }, [messages.length, pendingText]);

  async function submit() {
    const text = draft.trim();
    if (!text || busy) return;
    setDraft("");
    await onSend(text);
  }

  function onKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
      event.preventDefault();
      void submit();
    }
  }

  return (
    <section className="chat">
      {models && (
        <div className="chat-header">
          <label>
            Модель{" "}
            <select
              value={models.selected}
              disabled={busy}
              onChange={(e) => onModelChange(e.target.value)}
            >
              {models.options.map((m) => (
                <option key={m.id} value={m.id}>
                  {m.label}
                </option>
              ))}
            </select>
          </label>
        </div>
      )}
      <div className="chat-log">
        {messages.length === 0 && !pendingText && (
          <p className="muted">
            Напишите Claude, например: «Разбей историю на сцены» или «Сделай третью сцену короче».
          </p>
        )}
        {messages.map((m) => (
          <div key={m.id} className={`bubble ${m.role}`}>
            <p>{m.content}</p>
            {m.scenes_changed && <span className="badge">сцены обновлены</span>}
          </div>
        ))}
        {pendingText && (
          <>
            <div className="bubble user">
              <p>{pendingText}</p>
            </div>
            <div className="bubble assistant typing">Claude думает…</div>
          </>
        )}
        <div ref={endRef} />
      </div>
      <div className="chat-input">
        <textarea
          rows={2}
          placeholder="Сообщение Claude (Enter — отправить, Shift+Enter — новая строка)"
          value={draft}
          disabled={busy}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={onKeyDown}
        />
        <button disabled={busy || !draft.trim()} onClick={() => void submit()}>
          Отправить
        </button>
      </div>
    </section>
  );
}
