import { useCallback, useEffect, useState } from "react";
import { api, type Scene, type Story } from "./api";
import { SceneList } from "./SceneList";
import { StoryForm } from "./StoryForm";
import "./App.css";

function errorMessage(err: unknown): string {
  return typeof err === "string" ? err : String(err);
}

export default function App() {
  const [stories, setStories] = useState<readonly Story[]>([]);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [scenes, setScenes] = useState<readonly Scene[]>([]);
  const [splitting, setSplitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const selected = stories.find((s) => s.id === selectedId) ?? null;

  const run = useCallback(async (action: () => Promise<void>) => {
    setError(null);
    try {
      await action();
    } catch (err) {
      setError(errorMessage(err));
    }
  }, []);

  const reload = useCallback(
    () => run(async () => setStories(await api.listStories())),
    [run],
  );

  useEffect(() => {
    void reload();
  }, [reload]);

  useEffect(() => {
    if (selectedId === null) {
      setScenes([]);
      return;
    }
    void run(async () => setScenes((await api.getStory(selectedId)).scenes));
  }, [selectedId, run]);

  const create = (title: string, text: string) =>
    run(async () => {
      const story = await api.createStory(title, text);
      await reload();
      setSelectedId(story.id);
    });

  const remove = (id: number) =>
    run(async () => {
      await api.deleteStory(id);
      if (selectedId === id) setSelectedId(null);
      await reload();
    });

  const split = (id: number) =>
    run(async () => {
      setSplitting(true);
      try {
        setScenes(await api.splitStory(id));
      } finally {
        setSplitting(false);
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
        {selected ? (
          <>
            <header>
              <h1>{selected.title}</h1>
              <button disabled={splitting} onClick={() => split(selected.id)}>
                {splitting ? "Claude думает…" : "Разбить на сцены"}
              </button>
            </header>
            <SceneList scenes={scenes} />
          </>
        ) : (
          <p className="muted">Выберите или добавьте историю.</p>
        )}
      </main>
    </div>
  );
}
