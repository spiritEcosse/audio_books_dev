import { useState } from "react";
import type { MediaInfo, Scene } from "./api";
import { SceneMedia } from "./SceneMedia";

interface Props {
  storyId: number;
  scenes: readonly Scene[];
  media: MediaInfo | null;
  disabled: boolean;
  onMediaChange: (action: () => Promise<MediaInfo>) => void;
}

const AUDIO_TAG = /(\[[^\]]*\])/;
const hasAudioTag = (text: string) => AUDIO_TAG.test(text);
const withoutAudioTags = (text: string) =>
  text.replace(/\s*\[[^\]]*\]\s*/g, " ").trim();

/** Narration text with audio tags like [softly] rendered dimmed so the spoken words stay readable. */
function NarrationText({ text, showTags }: { text: string; showTags: boolean }) {
  if (!showTags) return <p>{withoutAudioTags(text)}</p>;
  return (
    <p>
      {text.split(AUDIO_TAG).map((part, index) =>
        AUDIO_TAG.test(part) ? (
          <span key={index} className="audio-tag">{part}</span>
        ) : (
          part
        ),
      )}
    </p>
  );
}

export function SceneList({ storyId, scenes, media, disabled, onMediaChange }: Props) {
  const [showTags, setShowTags] = useState(true);
  if (scenes.length === 0) {
    return <p className="muted">Сцен пока нет. Попросите Claude в чате разбить историю на сцены.</p>;
  }
  return (
    <>
      {scenes.some((scene) => hasAudioTag(scene.text)) && (
        <label className="tag-toggle">
          <input type="checkbox" checked={showTags} onChange={(e) => setShowTags(e.target.checked)} />
          Показывать теги озвучки
        </label>
      )}
    <ol className="scenes">
      {scenes.map((scene) => (
        <li key={scene.id}>
          <NarrationText text={scene.text} showTags={showTags} />
          <p className="prompt">🖼 {scene.image_prompt}</p>
          {media && (
            <SceneMedia
              storyId={storyId}
              scene={scene}
              media={media}
              disabled={disabled}
              onChange={onMediaChange}
            />
          )}
        </li>
      ))}
    </ol>
    </>
  );
}
