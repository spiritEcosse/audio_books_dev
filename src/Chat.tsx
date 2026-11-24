import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import type { Message } from "./api";

interface Props {
  messages: readonly Message[];
  pendingText: string | null;
  busy: boolean;
  onSend: (text: string) => Promise<void>;
  onAccept: (messageId: number) => void;
  onReject: (messageId: number) => void;
}

const PROPOSAL_LABEL = {
  pending: "Claude предлагает новый текст истории",
  accepted: "Текст истории сохранён",
  rejected: "Предложение отклонено",
} as const;

export function Chat({ messages, pendingText, busy, onSend, onAccept, onReject }: Props) {
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
            {m.proposal && (
              <div className={`proposal ${m.proposal.state}`}>
                <strong>{PROPOSAL_LABEL[m.proposal.state]}</strong>
                <details>
                  <summary>Показать текст</summary>
                  <p>{m.proposal.text}</p>
                </details>
                {m.proposal.state === "pending" && (
                  <div className="proposal-actions">
                    <button disabled={busy} onClick={() => onAccept(m.id)}>
                      Сохранить как текст истории
                    </button>
                    <button className="secondary" disabled={busy} onClick={() => onReject(m.id)}>
                      Отклонить
                    </button>
                  </div>
                )}
              </div>
            )}
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
