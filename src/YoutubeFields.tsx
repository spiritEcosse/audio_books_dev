import { useState } from "react";
import type { YoutubeForm, YoutubeVisibility } from "./api";
import { addTags, fromLocalInput, toLocalInput } from "./youtubeForm";

const VISIBILITY: readonly (readonly [YoutubeVisibility, string])[] = [
  ["private", "Приватное"],
  ["unlisted", "По ссылке"],
  ["public", "Опубликовать сразу"],
  ["scheduled", "По расписанию"],
];

interface Props {
  form: YoutubeForm;
  disabled: boolean;
  onChange: (changes: Partial<YoutubeForm>) => void;
}

function Question(props: {
  name: string;
  label: string;
  value: boolean | null;
  onChange: (value: boolean) => void;
}) {
  return (
    <fieldset className="youtube-question">
      <legend>{props.label}</legend>
      {[true, false].map((answer) => (
        <label key={String(answer)}>
          <input
            type="radio"
            name={props.name}
            checked={props.value === answer}
            onChange={() => props.onChange(answer)}
          />
          {answer ? "Да" : "Нет"}
        </label>
      ))}
    </fieldset>
  );
}

function Tags({ tags, onChange }: { tags: readonly string[]; onChange: (tags: string[]) => void }) {
  const [typed, setTyped] = useState("");
  const commit = () => {
    if (typed.trim()) onChange(addTags(tags, typed));
    setTyped("");
  };
  return (
    <div className="youtube-tags">
      <span>Теги</span>
      <ul>
        {tags.map((tag) => (
          <li key={tag}>
            {tag}
            <button
              type="button"
              className="danger"
              aria-label={`Убрать тег ${tag}`}
              onClick={() => onChange(tags.filter((other) => other !== tag))}
            >
              ×
            </button>
          </li>
        ))}
      </ul>
      <input
        value={typed}
        aria-label="Новый тег"
        placeholder="Новый тег и Enter"
        onChange={(e) => setTyped(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key !== "Enter") return;
          e.preventDefault();
          commit();
        }}
      />
    </div>
  );
}

/** The editable choices for a video. */
export function YoutubeFields({ form, disabled, onChange }: Props) {
  return (
    <fieldset className="youtube-fields" disabled={disabled}>
      <label>
        Название ({form.title.length}/100)
        <input value={form.title} maxLength={100} onChange={(e) => onChange({ title: e.target.value })} />
      </label>
      <label>
        Описание
        <textarea
          rows={6}
          value={form.description}
          onChange={(e) => onChange({ description: e.target.value })}
        />
      </label>
      <Tags tags={form.tags} onChange={(tags) => onChange({ tags })} />
      <p className="muted small">Язык видео и субтитров: английский.</p>
      <Question
        name="youtube-kids"
        label="Видео для детей?"
        value={form.made_for_kids}
        onChange={(made_for_kids) => onChange({ made_for_kids })}
      />
      <Question
        name="youtube-synthetic"
        label="Есть изменённый или синтетический контент?"
        value={form.synthetic_media}
        onChange={(synthetic_media) => onChange({ synthetic_media })}
      />
      <p className="muted small">
        У детских видео YouTube сам отключает комментарии; у остальных действуют настройки канала.
      </p>
      <label>
        Доступ
        <select
          value={form.visibility}
          onChange={(e) => onChange({ visibility: e.target.value as YoutubeVisibility })}
        >
          {VISIBILITY.map(([value, label]) => (
            <option key={value} value={value}>
              {label}
            </option>
          ))}
        </select>
      </label>
      {form.visibility === "scheduled" && (
        <label>
          Время публикации (местное)
          <input
            type="datetime-local"
            value={toLocalInput(form.publish_at)}
            onChange={(e) => onChange({ publish_at: fromLocalInput(e.target.value) })}
          />
        </label>
      )}
      <label className="youtube-check">
        <input
          type="checkbox"
          checked={form.subtitles}
          onChange={(e) => onChange({ subtitles: e.target.checked })}
        />
        Загрузить субтитры
      </label>
    </fieldset>
  );
}
