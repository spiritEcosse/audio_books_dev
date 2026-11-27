import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useCallback, useEffect, useRef, useState } from "react";
import {
  api,
  type Resolution,
  type YoutubeForm,
  type YoutubeInfo,
  type YoutubeProgress,
  type YoutubeUpload,
} from "./api";
import { YoutubeFields } from "./YoutubeFields";
import { blockers, percentLabel, visibilityText } from "./youtubeForm";

const EVENT = "youtube-progress";
const SAVE_DELAY_MS = 500;
/** How often to look at an upload that was started before the panel was opened. */
const POLL_MS = 3000;
/** How stale "now" may be when checking a scheduled time; the backend checks it again. */
const CLOCK_MS = 30_000;

type Work = "connect" | "draft" | "upload" | "captions" | "disconnect";

interface Props {
  storyId: number;
  /** Changes when the exported videos change, to look again. */
  videosKey: string;
}

function UploadResult(props: { upload: YoutubeUpload; busy: boolean; onRetry: () => void }) {
  const { upload } = props;
  const open = (url: string) => void openUrl(url);
  return (
    <div className="youtube-result">
      {upload.unfinished && <p>Загрузка не закончена: «Загрузить» продолжит с того же места.</p>}
      {upload.video_id && (
        <>
          <p>
            <strong>На YouTube ({upload.resolution}):</strong>{" "}
            {visibilityText(upload.privacy, upload.publish_at)}
          </p>
          <div className="media-actions">
            <button type="button" onClick={() => open(`https://youtu.be/${upload.video_id}`)}>
              Открыть видео
            </button>
            <button
              type="button"
              onClick={() => open(`https://studio.youtube.com/video/${upload.video_id}/edit`)}
            >
              Открыть в Studio
            </button>
          </div>
          {upload.subtitles === "uploaded" && <p>Субтитры загружены.</p>}
          {upload.subtitles === "pending" && (
            <p className="error">
              Субтитры не загружены. {upload.captions_error}{" "}
              <button type="button" disabled={props.busy} onClick={props.onRetry}>
                Повторить только субтитры
              </button>
            </p>
          )}
        </>
      )}
    </div>
  );
}

/** Connect the account, fill in the form and upload an exported video. */
export function YoutubePanel({ storyId, videosKey }: Props) {
  const [info, setInfo] = useState<YoutubeInfo | null>(null);
  const [form, setForm] = useState<YoutubeForm | null>(null);
  const [chosen, setChosen] = useState<Resolution | null>(null);
  const [work, setWork] = useState<Work | null>(null);
  const [percent, setPercent] = useState(0);
  const [again, setAgain] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const edited = useRef(false);
  const [now, setNow] = useState(() => Date.now() / 1000);

  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now() / 1000), CLOCK_MS);
    return () => clearInterval(timer);
  }, []);

  const reload = useCallback(async () => {
    try {
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
  }, [reload, videosKey]);

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

  if (!info || !form) return error ? <p className="error">{error}</p> : null;

  const change = (changes: Partial<YoutubeForm>) => {
    edited.current = true;
    setForm((current) => current && { ...current, ...changes });
  };
  const resolutions = info.videos.map((video) => video.label);
  const resolution = chosen && resolutions.includes(chosen) ? chosen : resolutions[resolutions.length - 1];
  const uploading = work === "upload" || info.uploading;
  const reasons = blockers(form, {
    connected: info.connected,
    hasVideo: resolution !== undefined,
    alreadyUploaded: info.upload?.already_uploaded ?? false,
    confirmedAgain: again,
    now,
  });

  const draft = () =>
    perform("draft", async () => change(await api.draftYoutube(storyId)));
  const upload = () =>
    perform("upload", async () => {
      if (!resolution) return;
      setPercent(0);
      await api.uploadYoutube(storyId, resolution, form, again);
      setAgain(false);
    });

  return (
    <details className="youtube-panel">
      <summary>YouTube{uploading ? ` · загрузка ${percentLabel(percent)}` : ""}</summary>
      <div className="media-actions">
        {info.connected ? (
          <>
            <span className="status ready">Аккаунт подключён</span>
            <button
              type="button"
              disabled={work !== null || uploading}
              onClick={() => void perform("disconnect", api.disconnectYoutube)}
            >
              Отключить
            </button>
          </>
        ) : work === "connect" ? (
          <>
            <span role="status">Разрешите доступ в браузере…</span>
            <button type="button" onClick={() => void api.cancelYoutubeConnect()}>
              Отмена
            </button>
          </>
        ) : (
          <button type="button" disabled={work !== null} onClick={() => void perform("connect", api.connectYoutube)}>
            Подключить YouTube
          </button>
        )}
        <button type="button" disabled={work !== null || uploading} onClick={() => void draft()}>
          {work === "draft" ? "Claude пишет…" : "Черновик от Claude (заменит название, описание и теги)"}
        </button>
      </div>
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
      {info.upload?.already_uploaded && !uploading && (
        <label className="youtube-check">
          <input type="checkbox" checked={again} onChange={(e) => setAgain(e.target.checked)} />
          Эта история уже есть на канале. Загрузить ещё одно видео.
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
            Загрузить{resolution ? ` ${resolution}` : ""} на YouTube
          </button>
          {reasons.length > 0 && <span className="muted small">Чтобы загрузить: {reasons.join("; ")}.</span>}
        </div>
      )}
      {error && <p className="error">{error}</p>}
      {info.upload && !uploading && (
        <UploadResult
          upload={info.upload}
          busy={work !== null}
          onRetry={() => void perform("captions", () => api.retryYoutubeCaptions(storyId))}
        />
      )}
    </details>
  );
}
