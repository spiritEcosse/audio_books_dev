import type { Scene } from "./api";

export function SceneList({ scenes }: { scenes: readonly Scene[] }) {
  if (scenes.length === 0) {
    return <p className="muted">Сцен пока нет. Нажмите «Разбить на сцены».</p>;
  }
  return (
    <ol className="scenes">
      {scenes.map((scene) => (
        <li key={scene.id}>
          <p>{scene.text}</p>
          <p className="prompt">🖼 {scene.image_prompt}</p>
        </li>
      ))}
    </ol>
  );
}
