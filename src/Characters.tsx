import { convertFileSrc } from "@tauri-apps/api/core";
import { useState } from "react";
import { api, MAX_CHARACTERS, type MediaInfo } from "./api";
import { type MediaChange, formatCredits } from "./mediaCost";
import { readBytes } from "./SceneMedia";

interface Props {
  media: MediaInfo;
  disabled: boolean;
  onChange: MediaChange;
}

/** The story's recurring characters: each picture is sent to the image model with every scene it is switched on for. */
export function Characters({ media, disabled, onChange }: Props) {
  const [name, setName] = useState("");
  const [file, setFile] = useState<File | null>(null);
  const [prompt, setPrompt] = useState("");
  const [inputKey, setInputKey] = useState(0);
  const full = media.characters.length >= MAX_CHARACTERS;
  const canAdd = !disabled && !full && name.trim() !== "" && file !== null;
  const canGenerate = !disabled && !full && name.trim() !== "" && prompt.trim() !== "";
  const generationCredits = media.image_models.find((model) => model.id === media.image_model)?.credits ?? 0;

  const generate = () => {
    const label = name.trim();
    const description = prompt.trim();
    setName("");
    setPrompt("");
    onChange(() => api.generateCharacter(media.story_id, label, description), {
      key: "panel:character-generate",
      label: "Генерирую персонажа… около минуты",
    });
  };

  const add = () => {
    if (!file) return;
    const picked = file;
    const label = name.trim();
    setName("");
    setFile(null);
    setInputKey((key) => key + 1);
    onChange(async () => api.addCharacter(media.story_id, label, picked.name, await readBytes(picked)), {
      key: "panel:character",
      label: "Сохраняю персонажа…",
    });
  };

  return (
    <details className="characters" open={media.characters.length > 0}>
      <summary>Персонажи ({media.characters.length})</summary>
      <p className="muted small">
        Картинка персонажа уходит в генерацию каждой сцены как референс. В строке сцены её можно выключить. Пока
        есть персонажи, картинка сцены 1 как образец стиля не используется.
      </p>
      <ul>
        {media.characters.map((character) => (
          <li key={character.id}>
            <img className="scene-reference" src={convertFileSrc(character.file_path)} alt={character.name} />
            <strong>{character.name}</strong>
            <button
              type="button"
              disabled={disabled}
              title="Удалить персонажа"
              onClick={() => onChange(() => api.removeCharacter(media.story_id, character.id))}
            >
              ×
            </button>
          </li>
        ))}
      </ul>
      <div className="character-form">
        <input
          type="text"
          placeholder="Имя персонажа"
          value={name}
          maxLength={60}
          disabled={disabled || full}
          onChange={(e) => setName(e.target.value)}
        />
        <input
          key={inputKey}
          type="file"
          accept="image/png,image/jpeg,image/webp"
          disabled={disabled || full}
          onChange={(e) => setFile(e.target.files?.[0] ?? null)}
        />
        <button type="button" disabled={!canAdd} onClick={add}>
          Добавить
        </button>
        {full && <span className="muted small">Не больше {MAX_CHARACTERS} персонажей</span>}
      </div>
      <div className="character-form">
        <textarea
          rows={2}
          placeholder="Или опишите персонажа и сгенерируйте: маленький милый слонёнок с большими розовыми ушами, сказочный мультяшный стиль, простой светлый фон, в полный рост"
          value={prompt}
          maxLength={1000}
          disabled={disabled || full}
          onChange={(e) => setPrompt(e.target.value)}
        />
        <button
          type="button"
          disabled={!canGenerate}
          title="Картинка создаётся моделью картинок истории, как сцена. Имя берётся из поля выше. Кредиты тратятся только после нажатия"
          onClick={generate}
        >
          Сгенерировать (≈{formatCredits(generationCredits)} кр.)
        </button>
      </div>
    </details>
  );
}
