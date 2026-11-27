import type { YoutubeForm } from "./api";

const MS = 1000;

/** The tags with those typed added: commas separate, blanks and repeats are skipped. */
export function addTags(tags: readonly string[], typed: string): string[] {
  const added = typed
    .split(",")
    .map((tag) => tag.trim())
    .filter(Boolean);
  return [...new Set([...tags, ...added])];
}

/** Unix seconds from a `datetime-local` value, which is in the local time zone. */
export function fromLocalInput(value: string): number | null {
  const millis = new Date(value).getTime();
  return Number.isNaN(millis) ? null : Math.floor(millis / MS);
}

export function toLocalInput(unix: number | null): string {
  if (unix === null) return "";
  const date = new Date(unix * MS);
  const two = (part: number) => String(part).padStart(2, "0");
  const day = `${date.getFullYear()}-${two(date.getMonth() + 1)}-${two(date.getDate())}`;
  return `${day}T${two(date.getHours())}:${two(date.getMinutes())}`;
}

export interface UploadConditions {
  readonly connected: boolean;
  readonly hasVideo: boolean;
  readonly alreadyUploaded: boolean;
  readonly confirmedAgain: boolean;
  /** Unix seconds. */
  readonly now: number;
}

/** Why the upload button is off; empty when it can be pressed. The backend checks the YouTube limits. */
export function blockers(form: YoutubeForm, conditions: UploadConditions): string[] {
  const checks: [boolean, string][] = [
    [!conditions.connected, "подключите аккаунт YouTube"],
    [!conditions.hasVideo, "соберите видео"],
    [form.title.trim() === "", "введите название"],
    [form.made_for_kids === null, "укажите, для детей ли видео"],
    [form.synthetic_media === null, "укажите про изменённый или синтетический контент"],
    [
      form.visibility === "scheduled" && (form.publish_at === null || form.publish_at <= conditions.now),
      "выберите время публикации в будущем",
    ],
    [conditions.alreadyUploaded && !conditions.confirmedAgain, "подтвердите повторную загрузку"],
  ];
  return checks.filter(([blocked]) => blocked).map(([, reason]) => reason);
}

/** The access of an uploaded video as YouTube returned it. */
export function visibilityText(privacy: string | null, publishAt: string | null): string {
  if (publishAt) return `запланировано на ${new Date(publishAt).toLocaleString("ru-RU")}`;
  if (privacy === "public") return "опубликовано, видно всем";
  if (privacy === "unlisted") return "доступно по ссылке";
  if (privacy === "private") return "приватное (видно только вам)";
  return "YouTube не сообщил доступ";
}

export function percentLabel(percent: number): string {
  return `${Math.floor(percent)}%`;
}
