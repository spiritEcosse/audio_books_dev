import { openUrl } from "@tauri-apps/plugin-opener";
import type { YoutubeChannel, YoutubeChannelUpload } from "./api";
import { visibilityText } from "./youtubeForm";

interface Props {
  uploads: readonly YoutubeChannelUpload[];
  channels: readonly YoutubeChannel[];
  busy: boolean;
  onRetry: (channelId: string) => void;
}

function UploadResult(props: {
  upload: YoutubeChannelUpload;
  channel: YoutubeChannel | undefined;
  busy: boolean;
  onRetry: () => void;
}) {
  const { upload, channel } = props;
  const open = (url: string) => void openUrl(url);
  const name = channel ? `Канал «${channel.title}»` : "Отключённый канал";
  return (
    <div className="youtube-result">
      {upload.unfinished && (
        <p>{name}: загрузка не закончена, «Загрузить» на этот канал продолжит с того же места.</p>
      )}
      {upload.video_id && (
        <>
          <p>
            <strong>
              {name} ({upload.resolution}):
            </strong>{" "}
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
              <button type="button" disabled={props.busy || !channel?.connected} onClick={props.onRetry}>
                Повторить только субтитры
              </button>
            </p>
          )}
        </>
      )}
    </div>
  );
}

/** What has been uploaded from this story, channel by channel. */
export function YoutubeUploads({ uploads, channels, busy, onRetry }: Props) {
  return uploads.map((upload) => (
    <UploadResult
      key={upload.channel_id}
      upload={upload}
      channel={channels.find((channel) => channel.id === upload.channel_id)}
      busy={busy}
      onRetry={() => onRetry(upload.channel_id)}
    />
  ));
}
