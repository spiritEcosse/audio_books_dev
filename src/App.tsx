import { useCallback, useEffect, useState } from "react";
import { api, type Story, type StoryDetail } from "./api";
import { Chat } from "./Chat";
import { SceneList } from "./SceneList";
import { StoryForm } from "./StoryForm";
import "./App.css";

function errorMessage(err: unknown): string {
  return typeof err === "string" ? err : String(err);
}

export default function App() {
  const [stories, setStories] = useState<readonly Story[]>([]);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [detail, setDetail] = useState<StoryDetail | null>(null);
  const [pendingText, setPendingText] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const run = useCallback(async (action: () => Promise<void>) => {
    setError(null);
    try {
      await action();
    } catch (err) {
      setError(errorMessage(err));
    }
  }, []);

  const reloadStories = useCallback(
    () => run(async () => setStories(await api.listStories())),
    [run],
  );

  useEffect(() => {
    void reloadStories();
  }, [reloadStories]);

  useEffect(() => {
    setDetail(null);
    if (selectedId === null) return;
    void run(async () => setDetail(await api.getStory(selectedId)));
  }, [selectedId, run]);

  const create = (title: string, text: string) =>
    run(async () => {
      const story = await api.createStory(title, text);
      await reloadStories();
      setSelectedId(story.id);
    });

  const remove = (id: number) =>
    run(async () => {
      await api.deleteStory(id);
      if (selectedId === id) setSelectedId(null);
      await reloadStories();
    });

  const send = (text: string) =>
    run(async () => {
      if (selectedId === null) return;
      setPendingText(text);
      try {
        setDetail(await api.sendMessage(selectedId, text));
      } catch (err) {
        // The user message is saved even when Claude fails, so show what is stored.
        setDetail(await api.getStory(selectedId));
        throw err;
      } finally {
        setPendingText(null);
      }
    });

  return (
    <div className="layout">
      <aside>
        <h2>Истории</h2>
        <ul className="stories">
          {stories.map((story) => (
            <li key={story.id} className={story.id === selectedId ? "active" : ""}>
              <button className="link" onClick={() => setSelectedId(story.id)}>
                {story.title}
              </button>
              <button className="danger" onClick={() => remove(story.id)} aria-label="Удалить">
                ×
              </button>
            </li>
          ))}
        </ul>
        <StoryForm onCreate={create} />
      </aside>
      <main>
        {error && <p className="error">{error}</p>}
        {detail ? (
          <div className="workspace">
            <div className="scenes-pane">
              <h1>{detail.story.title}</h1>
              <SceneList scenes={detail.scenes} />
            </div>
            <Chat
              messages={detail.messages}
              pendingText={pendingText}
              busy={pendingText !== null}
              onSend={send}
            />
          </div>
        ) : (
          <p className="muted">Выберите или добавьте историю.</p>
        )}
      </main>
    </div>
  );
}
