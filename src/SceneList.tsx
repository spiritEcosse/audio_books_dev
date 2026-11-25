import type { MediaInfo, Scene } from "./api";
import { SceneMedia } from "./SceneMedia";

interface Props {
  storyId: number;
  scenes: readonly Scene[];
  media: MediaInfo | null;
  disabled: boolean;
  onMediaChange: (action: () => Promise<MediaInfo>) => void;
}

export function SceneList({ storyId, scenes, media, disabled, onMediaChange }: Props) {
  if (scenes.length === 0) {
    return <p className="muted">Сцен пока нет. Попросите Claude в чате разбить историю на сцены.</p>;
  }
  return (
    <ol className="scenes">
      {scenes.map((scene) => (
        <li key={scene.id}>
          <p>{scene.text}</p>
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
  );
}
