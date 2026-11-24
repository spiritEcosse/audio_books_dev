import type { ModelsInfo } from "./api";

interface Props {
  models: ModelsInfo | null;
  disabled: boolean;
  onChange: (id: string) => void;
}

export function ModelPicker({ models, disabled, onChange }: Props) {
  if (!models) return null;
  return (
    <label className="model-picker">
      Модель Claude
      <select
        value={models.selected}
        disabled={disabled}
        onChange={(e) => onChange(e.target.value)}
      >
        {models.options.map((m) => (
          <option key={m.id} value={m.id}>
            {m.label}
          </option>
        ))}
      </select>
    </label>
  );
}
