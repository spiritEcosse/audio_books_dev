import type { YoutubeChannel } from "./api";

/** A channel's picture and name, so that channels are not confused. */
export function ChannelBadge({ channel }: { channel: YoutubeChannel }) {
  return (
    <span className="channel-badge">
      {channel.thumbnail && <img src={channel.thumbnail} alt="" referrerPolicy="no-referrer" />}
      <strong>{channel.title}</strong>
    </span>
  );
}
