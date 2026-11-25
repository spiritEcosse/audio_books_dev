import { convertFileSrc } from "@tauri-apps/api/core";
import { api, type AssetKind, type MediaInfo, type Scene, type SceneAsset } from "./api";
import { findAsset, formatCredits, sceneCredits } from "./mediaCost";

interface Props {
  storyId: number;
  scene: Scene;
  media: MediaInfo;
  disabled: boolean;
  onChange: (action: () => Promise<MediaInfo>) => void;
}

const KIND_LABEL: Record<AssetKind, string> = { image: "Картинка", audio: "Озвучка" };

function statusText(asset: SceneAsset | undefined): string {
  if (!asset) return "не создано";
  if (asset.status === "failed") return `ошибка: ${asset.error ?? "неизвестно"}`;
  if (asset.status === "requested") return asset.in_progress ? "генерируется…" : "в очереди";
  return asset.stale ? "устарело: текст сцены изменился" : "готово";
}

function Preview({ kind, path }: { kind: AssetKind; path: string }) {
  const src = convertFileSrc(path);
  return kind === "image" ? (
    <img className="scene-image" src={src} alt="Иллюстрация сцены" />
  ) : (
    <audio className="scene-audio" controls src={src} />
  );
}

function AssetRow({ kind, scene, storyId, media, disabled, onChange }: Props & { kind: AssetKind }) {
  const asset = findAsset(media, scene.position, kind);
  const queued = asset?.status === "requested";
  return (
    <div className="asset">
      <div className="asset-head">
        <strong>{KIND_LABEL[kind]}</strong>
        <span className={`status ${asset?.status ?? "none"}`}>{statusText(asset)}</span>
        <span className="muted">
          {asset?.estimate_credits != null
            ? `${formatCredits(asset.estimate_credits)} кр. (точно)`
            : `≈${formatCredits(sceneCredits(media, scene, kind))} кр.`}
        </span>
        <button
          disabled={disabled || queued}
          onClick={() => onChange(() => api.requestAsset(storyId, scene.position, kind))}
        >
          {asset?.file_path ? "Перегенерировать" : "Запросить"}
        </button>
      </div>
      {asset?.file_path && <Preview kind={kind} path={asset.file_path} />}
    </div>
  );
}

async function readBytes(file: File): Promise<number[]> {
  return Array.from(new Uint8Array(await file.arrayBuffer()));
}

function ReferenceRow({ scene, storyId, media, disabled, onChange }: Props) {
  const reference = media.references.find((ref) => ref.position === scene.position);
  const fileName = reference?.file_path.split(/[\\/]/).pop();
  return (
    <div className="asset reference">
      <label>
        Референс (необязательно)
        <input
          type="file"
          accept="image/png,image/jpeg,image/webp"
          disabled={disabled}
          onChange={(e) => {
            const file = e.target.files?.[0];
            e.target.value = "";
            if (file) {
              onChange(async () =>
                api.setSceneReference(storyId, scene.position, file.name, await readBytes(file)),
              );
            }
          }}
        />
      </label>
      {reference && (
        <span>
          {fileName}{" "}
          <button disabled={disabled} onClick={() => onChange(() => api.clearSceneReference(storyId, scene.position))}>
            ×
          </button>
        </span>
      )}
      {reference && <img className="scene-reference" src={convertFileSrc(reference.file_path)} alt="Референс" />}
    </div>
  );
}

export function SceneMedia(props: Props) {
  return (
    <div className="scene-media">
      <AssetRow {...props} kind="image" />
      <ReferenceRow {...props} />
      <AssetRow {...props} kind="audio" />
    </div>
  );
}
