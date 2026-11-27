import { useCallback, useEffect, useState } from "react";
import { api, type MediaInfo, type ModelsInfo, type ProfilesInfo, type Story, type StoryDetail, type StorySettings } from "./api";
import { Chat } from "./Chat";
import { MediaPanel } from "./MediaPanel";
import { YoutubePanel } from "./YoutubePanel";
import { YoutubeSettings } from "./YoutubeSettings";
import { type MediaBusy, type MediaChange } from "./mediaCost";
import { Revisions } from "./Revisions";
import { ModelPicker } from "./ModelPicker";
import { SceneList } from "./SceneList";
import { StoryForm } from "./StoryForm";
import { StorySettingsPanel } from "./StorySettingsPanel";
import "./App.css";

const QUEUE_POLL_MS = 3000;

const GENERIC_BUSY: MediaBusy = { key: "media", label: "Выполняю…" };

function errorMessage(err: unknown): string {
  return typeof err === "string" ? err : String(err);
}

export default function App() {
  const [stories, setStories] = useState<readonly Story[]>([]);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [detail, setDetail] = useState<StoryDetail | null>(null);
  const [pendingText, setPendingText] = useState<string | null>(null);
  const [models, setModels] = useState<ModelsInfo | null>(null);
  const [profiles, setProfiles] = useState<ProfilesInfo | null>(null);
  const [media, setMedia] = useState<MediaInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [inSettings, setInSettings] = useState(false);
  const [settingsOpened, setSettingsOpened] = useState(false);
  const [channelsKey, setChannelsKey] = useState(0);
  const [mediaOps, setMediaOps] = useState<readonly MediaBusy[]>([]);

  const run = useCallback(async (action: () => Promise<void>) => {
    setError(null);
    try {
      await action();
    } catch (err) {
      setError(errorMessage(err));
    }
  }, []);

  const openSettings = () => {
    setSettingsOpened(true);
    setInSettings(true);
  };
  const showStory = (id: number) => {
    setInSettings(false);
    setSelectedId(id);
  };

  const reloadStories = useCallback(
    () => run(async () => setStories(await api.listStories())),
    [run],
  );

  useEffect(() => {
    void reloadStories();
  }, [reloadStories]);

  useEffect(() => {
    void run(async () => setModels(await api.getModels()));
  }, [run]);

  useEffect(() => {
    void run(async () => setProfiles(await api.listProfiles()));
  }, [run]);

  const changeSettings = (settings: StorySettings) =>
    run(async () => {
      if (selectedId === null) return;
      setDetail(await api.setStorySettings(selectedId, settings));
    });

  const changeModel = (id: string) =>
    run(async () => {
      await api.setModel(id);
      setModels((current) => (current ? { ...current, selected: id } : current));
    });

  useEffect(() => {
    setDetail(null);
    if (selectedId === null) return;
    void run(async () => setDetail(await api.getStory(selectedId)));
  }, [selectedId, run]);

  const detailStoryId = detail?.story.id ?? null;
  useEffect(() => {
    if (detailStoryId === null) {
      setMedia(null);
      return;
    }
    void run(async () => setMedia(await api.getMedia(detailStoryId)));
  }, [detail, detailStoryId, run]);

  // Claude fulfils queued requests outside the app, so keep checking while any are pending.
  const hasQueued = media?.assets.some((asset) => asset.status === "requested") ?? false;
  useEffect(() => {
    if (!hasQueued || detailStoryId === null) return;
    const timer = setInterval(() => {
      api.getMedia(detailStoryId).then(setMedia).catch((err) => setError(errorMessage(err)));
    }, QUEUE_POLL_MS);
    return () => clearInterval(timer);
  }, [hasQueued, detailStoryId]);

  const changeMedia: MediaChange = (action, busy = GENERIC_BUSY) => {
    setMediaOps((ops) => [...ops, busy]);
    void run(async () => setMedia(await action())).finally(() =>
      setMediaOps((ops) => ops.filter((op) => op !== busy)),
    );
  };

  const create = (title: string, text: string) =>
    run(async () => {
      const story = await api.createStory(title, text);
      await reloadStories();
      showStory(story.id);
    });

  const remove = (id: number) =>
    run(async () => {
      await api.deleteStory(id);
      if (selectedId === id) setSelectedId(null);
      await reloadStories();
    });

  const applyDetail = (action: () => Promise<StoryDetail>) =>
    run(async () => setDetail(await action()));

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
        <ModelPicker
          models={models}
          disabled={pendingText !== null}
          onChange={(id) => void changeModel(id)}
        />
        <h2>Истории</h2>
        <ul className="stories">
          {stories.map((story) => (
            <li key={story.id} className={!inSettings && story.id === selectedId ? "active" : ""}>
              <button className="link" onClick={() => showStory(story.id)}>
                {story.title}
              </button>
              <button className="danger" onClick={() => remove(story.id)} aria-label="Удалить">
                ×
              </button>
            </li>
          ))}
        </ul>
        <StoryForm onCreate={create} />
        <button type="button" className="open-settings" aria-pressed={inSettings} onClick={openSettings}>
          Настройки
        </button>
      </aside>
      <main>
        {mediaOps.map((op) => (
          <p key={op.key} className="busy-banner" role="status">
            <span className="spinner" aria-hidden="true" /> {op.label}
          </p>
        ))}
        {error && <p className="error">{error}</p>}
        {settingsOpened && (
          <YoutubeSettings visible={inSettings} onChange={() => setChannelsKey((key) => key + 1)} />
        )}
        {detail ? (
          // Hidden, not removed, so that the chat draft and running progress survive a visit to the settings.
          <div className="workspace" hidden={inSettings}>
            <div className="scenes-pane">
              <h1>{detail.story.title}</h1>
              <StorySettingsPanel
                settings={detail.settings}
                stats={detail.stats}
                profiles={profiles}
                disabled={pendingText !== null}
                onChange={(next) => void changeSettings(next)}
              />
              <details className="story-text">
                <summary>Текст истории</summary>
                <p>{detail.story.text}</p>
              </details>
              <Revisions
                storyId={detail.story.id}
                refreshKey={detail}
                disabled={pendingText !== null}
                onRestore={(id) => void applyDetail(() => api.restoreRevision(id))}
              />
              {media && media.story_id === detail.story.id && (
                <MediaPanel
                  media={media}
                  scenes={detail.scenes}
                  disabled={pendingText !== null}
                  busy={mediaOps}
                  onChange={changeMedia}
                />
              )}
              {media && media.story_id === detail.story.id && (
                <YoutubePanel
                  key={detail.story.id}
                  storyId={detail.story.id}
                  videosKey={media.video ?? ""}
                  channelsKey={channelsKey}
                  onOpenSettings={openSettings}
                />
              )}
              <SceneList
                storyId={detail.story.id}
                scenes={detail.scenes}
                media={media && media.story_id === detail.story.id ? media : null}
                disabled={pendingText !== null}
                busy={mediaOps}
                onMediaChange={changeMedia}
              />
            </div>
            <Chat
              messages={detail.messages}
              pendingText={pendingText}
              busy={pendingText !== null}
              onSend={send}
              onAccept={(id) => void applyDetail(() => api.acceptProposal(id))}
              onReject={(id) => void applyDetail(() => api.rejectProposal(id))}
            />
          </div>
        ) : (
          !inSettings && <p className="muted">Выберите или добавьте историю.</p>
        )}
      </main>
    </div>
  );
}
