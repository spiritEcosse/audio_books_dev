import { useCallback, useEffect, useState } from "react";
import { api, type YoutubeChannel, type YoutubeChannels } from "./api";
import { ChannelBadge } from "./ChannelBadge";

interface Props {
  /** The view stays mounted when left, so that a sign-in under way can still be cancelled. */
  visible: boolean;
  onChange: () => void;
}

/** The connected channels: add, reconnect and disconnect them. */
export function YoutubeSettings({ visible, onChange }: Props) {
  const [list, setList] = useState<YoutubeChannels | null>(null);
  const [connecting, setConnecting] = useState(false);
  const [disconnecting, setDisconnecting] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(async () => {
    try {
      setList(await api.youtubeChannels());
    } catch (err) {
      setError(String(err));
    }
  }, []);

  useEffect(() => {
    if (visible) void reload();
  }, [reload, visible]);

  const perform = async (setBusy: (busy: boolean) => void, action: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      await action();
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
      await reload();
      onChange();
    }
  };

  const connect = () =>
    perform(setConnecting, async () => {
      const channel = await api.connectYoutube();
      setNotice(`Подключён канал «${channel.title}».`);
    });
  const disconnect = (channel: YoutubeChannel) =>
    perform(setDisconnecting, () => api.disconnectYoutube(channel.id));
  const busy = connecting || disconnecting;

  return (
    <section className="app-settings" hidden={!visible}>
      <h1>Настройки</h1>
      <h2>Каналы YouTube</h2>
      {list?.warning && (
        <p className="error">
          {list.warning}{" "}
          <button
            type="button"
            disabled={connecting || disconnecting}
            onClick={() => void perform(setDisconnecting, api.forgetLegacyYoutube)}
          >
            Забыть старый вход (его загрузки получит первый канал)
          </button>
        </p>
      )}
      {list && list.channels.length === 0 && <p className="muted">Пока нет подключённых каналов.</p>}
      <ul className="channels">
        {list?.channels.map((channel) => (
          <li key={channel.id}>
            <ChannelBadge channel={channel} />
            <span className={`status ${channel.connected ? "ready" : "failed"}`}>
              {channel.connected ? "подключён" : "вход истёк"}
            </span>
            {!channel.connected && (
              <button type="button" disabled={busy} onClick={() => void connect()}>
                Переподключить
              </button>
            )}
            <button type="button" disabled={busy} onClick={() => void disconnect(channel)}>
              Отключить
            </button>
          </li>
        ))}
      </ul>
      {connecting ? (
        <div className="media-actions">
          <span role="status">Выберите канал и разрешите доступ в браузере…</span>
          <button type="button" onClick={() => void api.cancelYoutubeConnect()}>
            Отмена
          </button>
        </div>
      ) : (
        <button type="button" disabled={busy} onClick={() => void connect()}>
          Добавить канал
        </button>
      )}
      {notice && <p role="status">{notice}</p>}
      {error && <p className="error">{error}</p>}
    </section>
  );
}
