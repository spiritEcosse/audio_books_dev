import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";
import {
  api,
  type Resolution,
  type YoutubeChannels,
  type YoutubeForm,
  type YoutubeInfo,
  type YoutubeProgress,
} from "./api";
import { ChannelBadge } from "./ChannelBadge";
import { YoutubeFields } from "./YoutubeFields";
import { blockers, percentLabel, selectedChannel } from "./youtubeForm";
import { YoutubeUploads } from "./YoutubeUploads";

const EVENT = "youtube-progress";
const SAVE_DELAY_MS = 500;
/** How often to look at an upload that was started before the panel was opened. */
const POLL_MS = 3000;
/** How stale "now" may be when checking a scheduled time; the backend checks it again. */
const CLOCK_MS = 30_000;

type Work = "draft" | "upload" | "captions";

interface Props {
  storyId: number;
  /** Changes when the exported videos change, to look again. */
  videosKey: string;
  /** Changes when the settings were open, where channels come and go. */
  channelsKey: number;
  onOpenSettings: () => void;
}

/** Choose a channel, fill in the form and upload an exported video. */
export function YoutubePanel({ storyId, videosKey, channelsKey, onOpenSettings }: Props) {
  const [info, setInfo] = useState<YoutubeInfo | null>(null);
  const [list, setList] = useState<YoutubeChannels | null>(null);
  const [form, setForm] = useState<YoutubeForm | null>(null);
  const [chosen, setChosen] = useState<Resolution | null>(null);
  const [work, setWork] = useState<Work | null>(null);
  const [percent, setPercent] = useState(0);
  /** The channel a second upload was confirmed for. */
  const [againFor, setAgainFor] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const edited = useRef(false);
  const [now, setNow] = useState(() => Date.now() / 1000);

  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now() / 1000), CLOCK_MS);
    return () => clearInterval(timer);
  }, []);

  const reload = useCallback(async () => {
    try {
      // First, because it carries an older single account over to its channel and that account's uploads with it.
      setList(await api.youtubeChannels());
      const next = await api.getYoutube(storyId);
      setInfo(next);
      // What the user is typing is not overwritten.
      setForm((current) => (edited.current && current ? current : next.form));
    } catch (err) {
      setError(String(err));
    }
  }, [storyId]);

  useEffect(() => {
    void reload();
  }, [reload, videosKey, channelsKey]);

  useEffect(() => {
    if (!form || !edited.current) return;
    const timer = setTimeout(() => {
      api.saveYoutubeForm(storyId, form).catch((err) => setError(String(err)));
    }, SAVE_DELAY_MS);
    return () => clearTimeout(timer);
  }, [form, storyId]);

  useEffect(() => {
    let stopped = false;
    let unlisten: (() => void) | undefined;
    listen<YoutubeProgress>(EVENT, ({ payload }) => {
      if (payload.story_id === storyId) setPercent(payload.percent);
    }).then((stop) => (stopped ? stop() : (unlisten = stop)));
    return () => {
      stopped = true;
      unlisten?.();
    };
  }, [storyId]);

  const uploadingElsewhere = Boolean(info?.uploading) && work !== "upload";
  useEffect(() => {
    if (!uploadingElsewhere) return;
    const timer = setInterval(() => void reload(), POLL_MS);
    return () => clearInterval(timer);
  }, [uploadingElsewhere, reload]);

  const perform = async (kind: Work, action: () => Promise<unknown>) => {
    setWork(kind);
    setError(null);
    try {
      await action();
    } catch (err) {
      setError(String(err));
    } finally {
      setWork(null);
      await reload();
    }
  };

  if (!info || !form || !list) return error ? <p className="error">{error}</p> : null;

  const change = (changes: Partial<YoutubeForm>) => {
    edited.current = true;
    setForm((current) => current && { ...current, ...changes });
  };
  const resolutions = info.videos.map((video) => video.label);
  const resolution = chosen && resolutions.includes(chosen) ? chosen : resolutions[resolutions.length - 1];
  const uploading = work === "upload" || info.uploading;
  const channel = selectedChannel(list.channels, form.channel_id);
  const previous = info.uploads.find((upload) => upload.channel_id === channel?.id);
  const again = channel !== null && againFor === channel.id;
  const reasons = blockers(form, {
    channel,
    hasChannels: list.channels.length > 0,
    hasVideo: resolution !== undefined,
    alreadyUploaded: previous?.already_uploaded ?? false,
    confirmedAgain: again,
    now,
  });

  const draft = () =>
    perform("draft", async () => change(await api.draftYoutube(storyId)));
  const upload = () =>
    perform("upload", async () => {
      if (!resolution || !channel) return;
      setPercent(0);
      const chosenForm = { ...form, channel_id: channel.id };
      change(chosenForm);
      await api.uploadYoutube(storyId, resolution, chosenForm, again);
      setAgainFor(null);
    });

  return (
    <details className="youtube-panel">
      <summary>YouTube{uploading ? ` · загрузка ${percentLabel(percent)}` : ""}</summary>
      <div className="media-actions">
        <button type="button" disabled={work !== null || uploading} onClick={() => void draft()}>
          {work === "draft" ? "Claude пишет…" : "Черновик от Claude (заменит название, описание и теги)"}
        </button>
      </div>
      {list.warning && <p className="error">{list.warning}</p>}
      {list.channels.length === 0 ? (
        <p>
          Нет подключённых каналов.{" "}
          <button type="button" onClick={onOpenSettings}>
            Открыть настройки
          </button>
        </p>
      ) : (
        <div className="media-actions">
          <label>
            Канал{" "}
            <select
              value={channel?.id ?? ""}
              disabled={uploading}
              onChange={(e) => change({ channel_id: e.target.value })}
            >
              {!channel && <option value="">выберите канал</option>}
              {list.channels.map((option) => (
                <option key={option.id} value={option.id}>
                  {option.title}
                  {option.connected ? "" : " (вход истёк)"}
                </option>
              ))}
            </select>
          </label>
          {channel && <ChannelBadge channel={channel} />}
          <button type="button" onClick={onOpenSettings}>
            Настройки каналов
          </button>
        </div>
      )}
      <YoutubeFields form={form} disabled={uploading || work === "draft"} onChange={change} />
      {resolutions.length > 1 && (
        <label>
          Какое видео загрузить
          <select value={resolution} disabled={uploading} onChange={(e) => setChosen(e.target.value as Resolution)}>
            {resolutions.map((label) => (
              <option key={label} value={label}>
                {label}
              </option>
            ))}
          </select>
        </label>
      )}
      {channel && previous?.already_uploaded && !uploading && (
        <label className="youtube-check">
          <input
            type="checkbox"
            checked={again}
            onChange={(e) => setAgainFor(e.target.checked ? channel.id : null)}
          />
          Эта история уже есть на канале «{channel.title}». Загрузить ещё одно видео.
        </label>
      )}
      {uploading ? (
        <div className="export-progress" role="status">
          <progress max={100} value={percent} />
          <span>Загрузка на YouTube · {percentLabel(percent)}</span>
          <button type="button" onClick={() => void api.cancelYoutubeUpload()}>
            Отменить загрузку
          </button>
        </div>
      ) : (
        <div className="media-actions">
          <button type="button" disabled={work !== null || reasons.length > 0} onClick={() => void upload()}>
            Загрузить{resolution ? ` ${resolution}` : ""}
            {channel ? ` на канал «${channel.title}»` : " на YouTube"}
          </button>
          {reasons.length > 0 && <span className="muted small">Чтобы загрузить: {reasons.join("; ")}.</span>}
        </div>
      )}
      {error && <p className="error">{error}</p>}
      {!uploading && (
        <YoutubeUploads
          uploads={info.uploads}
          channels={list.channels}
          busy={work !== null}
          onRetry={(id) => void perform("captions", () => api.retryYoutubeCaptions(storyId, id))}
        />
      )}
    </details>
  );
}
