import { api, type MediaInfo, type Scene } from "./api";
import { BusyButton } from "./BusyButton";
import { type MediaBusy, type MediaChange, formatCredits, summarize } from "./mediaCost";

interface Props {
  media: MediaInfo;
  scenes: readonly Scene[];
  disabled: boolean;
  busy: readonly MediaBusy[];
  onChange: MediaChange;
}

export function MediaPanel({ media, scenes, disabled, busy, onChange }: Props) {
  const { missing, queued, queuedExact } = summarize(media, scenes);
  const locked = disabled || busy.length > 0;
  const unpriced = media.assets.filter(
    (a) => a.status === "requested" && !a.in_progress && a.estimate_credits == null,
  );
  const estimateAll = () =>
    api.estimateAudioQueue(media.story_id).then(() => api.estimateImageQueue(media.story_id));
  const selectedVoice = media.voices.find((voice) => voice.id === media.voice_id);
  return (
    <section className="media-panel">
      <label>
        Модель картинок
        <select
          value={media.image_model}
          disabled={locked}
          onChange={(e) => onChange(() => api.setImageModel(media.story_id, e.target.value))}
        >
          {media.image_models.map((model) => (
            <option key={model.id} value={model.id}>
              {model.label} — ≈{formatCredits(model.credits)} кр. за картинку
            </option>
          ))}
        </select>
      </label>
      <details className="voices">
        <summary>Голос озвучки: {selectedVoice?.label ?? "—"}</summary>
        <ul>
          {media.voices.map((voice) => (
            <li key={voice.id} className={voice.id === media.voice_id ? "active" : ""}>
              <label>
                <input
                  type="radio"
                  name="voice"
                  checked={voice.id === media.voice_id}
                  disabled={locked}
                  onChange={() => onChange(() => api.setVoice(media.story_id, voice.id))}
                />
                <strong>{voice.label}</strong>
              </label>
              <span className="muted">{voice.description}</span>
              <audio controls preload="none" src={voice.preview_url} />
            </li>
          ))}
        </ul>
      </details>
      <p>
        Не создано: {missing.items} (≈{formatCredits(missing.credits)} кр.) · В очереди: {queued.items} (
        {queuedExact || queued.items === 0 ? "" : "≈"}
        {formatCredits(queued.credits)} кр.
        {queued.items > 0 && (queuedExact ? ", точная оценка" : ", приблизительно: скажите Claude «оцени очередь»")})
      </p>
      <div className="media-actions">
        <BusyButton
          busy={busy}
          tag="panel:request"
          disabled={locked || missing.items === 0}
          onClick={() =>
            onChange(() => api.requestMissingAssets(media.story_id), {
              key: "panel:request",
              label: "Ставлю в очередь…",
            })
          }
        >
          Поставить в очередь всё недостающее
        </BusyButton>
        <BusyButton
          busy={busy}
          tag="panel:cancel"
          disabled={locked || queued.items === 0}
          onClick={() =>
            onChange(() => api.cancelQueue(media.story_id), { key: "panel:cancel", label: "Очищаю очередь…" })
          }
        >
          Очистить очередь
        </BusyButton>
        <BusyButton
          busy={busy}
          tag="panel:estimate"
          disabled={locked || unpriced.length === 0}
          onClick={() => onChange(estimateAll, { key: "panel:estimate", label: "Измеряю цены…" })}
        >
          Измерить точные цены{unpriced.length > 0 ? ` (${unpriced.length})` : ""}
        </BusyButton>
        <BusyButton
          busy={busy}
          tag="panel:refresh"
          disabled={locked}
          onClick={() =>
            onChange(() => api.getMedia(media.story_id), { key: "panel:refresh", label: "Обновляю…" })
          }
        >
          Обновить
        </BusyButton>
      </div>
      <p className="muted">
        «Поставить в очередь» в сцене запоминает запрос (цену показывает подсказка при наведении), «Сгенерировать (N кр.)» запускает (через Claude и коннектор ElevenLabs). Точную цену можно измерить в меню «▾». Картинки сцен 1+ берут за образец картинку сцены 0 и свой референс. Цены — оценка на{" "}
        {media.prices_measured_on}, озвучка ≈{media.speech_credits_per_char} кр. за символ. Файлы: {media.media_dir}
      </p>
    </section>
  );
}
