import { useEffect, useState } from "react";
import type { AgeGroup, ProfilesInfo, StorySettings, StoryStats } from "./api";

interface Props {
  settings: StorySettings;
  stats: StoryStats;
  profiles: ProfilesInfo | null;
  disabled: boolean;
  onChange: (settings: StorySettings) => void;
}

const AGE_LABELS: Record<AgeGroup, string> = {
  up_to_5: "До 5 лет",
  up_to_10: "До 10 лет",
  up_to_15: "До 15 лет",
  adult: "Взрослые",
};
const OVER_TARGET_RATIO = 1.15;

export function StorySettingsPanel({ settings, stats, profiles, disabled, onChange }: Props) {
  const [minutes, setMinutes] = useState(String(settings.target_minutes));

  useEffect(() => {
    setMinutes(String(settings.target_minutes));
  }, [settings.target_minutes]);

  const commitMinutes = () => {
    const value = Math.round(Number(minutes));
    if (Number.isFinite(value) && value > 0 && value !== settings.target_minutes) {
      onChange({ ...settings, target_minutes: value });
    } else {
      setMinutes(String(settings.target_minutes));
    }
  };

  const over = stats.estimated_minutes > settings.target_minutes * OVER_TARGET_RATIO;

  return (
    <div className="settings">
      <div className={`stats ${over ? "over" : ""}`}>
        ≈ {stats.words} слов · ≈ {stats.estimated_minutes} мин из {settings.target_minutes}
        {over && " (длиннее цели)"}
        <span className="muted small">
          {" "}· {stats.from_scenes ? "по сценам" : "по тексту истории"} · {stats.characters} знаков
        </span>
      </div>
      <div className="settings-row">
        <label>
          Скилл
          <select
            value={settings.profile}
            disabled={disabled || !profiles}
            onChange={(e) => onChange({ ...settings, profile: e.target.value })}
          >
            {(profiles?.options ?? []).map((p) => (
              <option key={p.id} value={p.id}>
                {p.label}
                {p.builtin ? "" : " (свой)"}
              </option>
            ))}
          </select>
        </label>
        <label>
          Возраст
          <select
            value={settings.age_group}
            disabled={disabled}
            onChange={(e) => onChange({ ...settings, age_group: e.target.value as AgeGroup })}
          >
            {(Object.keys(AGE_LABELS) as AgeGroup[]).map((age) => (
              <option key={age} value={age}>
                {AGE_LABELS[age]}
              </option>
            ))}
          </select>
        </label>
        <label>
          Длительность, мин
          <input
            type="number"
            min={1}
            max={600}
            value={minutes}
            disabled={disabled}
            onChange={(e) => setMinutes(e.target.value)}
            onBlur={commitMinutes}
            onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
          />
        </label>
      </div>
      {profiles && (
        <p className="muted small">
          Свои скиллы: положите файл .md в папку {profiles.custom_dir}. Смена скилла, возраста или
          длительности начинает новую сессию Claude с тем же контекстом.
        </p>
      )}
    </div>
  );
}
