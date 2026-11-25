import { api, type MediaInfo, type Scene } from "./api";
import { formatCredits, summarize } from "./mediaCost";
interface Props {
  media: MediaInfo;
  scenes: readonly Scene[];
  disabled: boolean;
  onChange: (action: () => Promise<MediaInfo>) => void;
}

export function MediaPanel({ media, scenes, disabled, onChange }: Props) {
  const { missing, queued, queuedExact } = summarize(media, scenes);
  const selectedVoice = media.voices.find((voice) => voice.id === media.voice_id);
  return (
    <section className="media-panel">
      <label>
        Модель картинок
        <select
          value={media.image_model}
          disabled={disabled}
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
                  disabled={disabled}
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
        <button
          disabled={disabled || missing.items === 0}
          onClick={() => onChange(() => api.requestMissingAssets(media.story_id))}
        >
          Запросить всё недостающее
        </button>
        <button disabled={disabled} onClick={() => onChange(() => api.getMedia(media.story_id))}>
          Обновить
        </button>
      </div>
      <p className="muted">
        Генерирует Claude через коннектор ElevenLabs: после запроса напишите ему «сделай очередь». Цены — оценка на{" "}
        {media.prices_measured_on}, озвучка ≈{media.speech_credits_per_char} кр. за символ. Файлы: {media.media_dir}
      </p>
    </section>
  );
}
