import { invoke } from "@tauri-apps/api/core";

export interface Story {
  id: number;
  title: string;
  text: string;
}

export interface Scene {
  id: number;
  position: number;
  text: string;
  image_prompt: string;
}

export interface Proposal {
  text: string;
  state: "pending" | "accepted" | "rejected";
}

export interface Message {
  id: number;
  role: "user" | "assistant";
  content: string;
  scenes_changed: boolean;
  proposal: Proposal | null;
}

export interface Revision {
  id: number;
  kind: "story_text" | "scenes";
  created_at: string;
  summary: string;
}

export type AgeGroup = "up_to_5" | "up_to_10" | "up_to_15" | "adult";

export interface StorySettings {
  target_minutes: number;
  age_group: AgeGroup;
  profile: string;
}

export interface StoryStats {
  words: number;
  characters: number;
  estimated_minutes: number;
  from_scenes: boolean;
}

export interface ProfileOption {
  id: string;
  label: string;
  builtin: boolean;
}

export interface ProfilesInfo {
  options: ProfileOption[];
  custom_dir: string;
}

export interface StoryDetail {
  story: Story;
  scenes: Scene[];
  messages: Message[];
  settings: StorySettings;
  stats: StoryStats;
}

export type AssetKind = "image" | "audio";

/** Sent as the `export-progress` event while a video export runs. */
export interface ExportProgress {
  story_id: number;
  percent: number;
  stage: "clip" | "join";
  scene: number;
  scenes: number;
}

export interface SceneAsset {
  position: number;
  kind: AssetKind;
  status: "requested" | "ready" | "failed";
  file_path: string | null;
  stale: boolean;
  model_id: string;
  voice_id: string | null;
  credits: number | null;
  estimate_credits: number | null;
  in_progress: boolean;
  /** Launched and paid for, result not received yet: collect it, do not generate again. */
  awaiting_result: boolean;
  error: string | null;
  upscaled: string | null;
}

export interface SceneReference {
  position: number;
  file_path: string;
}

/** A recurring character: its picture goes to the image model with every scene it is not switched off for. */
export interface Character {
  id: number;
  name: string;
  file_path: string;
}

export interface SceneCharacterOff {
  position: number;
  character_id: number;
}

/** Mirrors `characters::MAX_CHARACTERS` in the backend. */
export const MAX_CHARACTERS = 4;

export interface ImageModel {
  id: string;
  label: string;
  credits: number;
}

export interface Voice {
  id: string;
  label: string;
  description: string;
  preview_url: string;
}

export interface MediaInfo {
  story_id: number;
  image_model: string;
  image_models: ImageModel[];
  voice_id: string;
  voices: Voice[];
  speech_credits_per_char: number;
  prices_measured_on: string;
  media_dir: string;
  assets: SceneAsset[];
  references: SceneReference[];
  characters: Character[];
  character_off: SceneCharacterOff[];
  video: string | null;
}

export interface ModelOption {
  id: string;
  label: string;
}

export interface ModelsInfo {
  options: ModelOption[];
  selected: string;
}

export type YoutubeVisibility = "private" | "unlisted" | "public" | "scheduled";

export interface YoutubeForm {
  title: string;
  description: string;
  tags: string[];
  visibility: YoutubeVisibility;
  /** Unix seconds. */
  publish_at: number | null;
  made_for_kids: boolean | null;
  synthetic_media: boolean | null;
  subtitles: boolean;
  /** The channel the video goes to. */
  channel_id: string | null;
}

export interface YoutubeChannel {
  id: string;
  title: string;
  thumbnail: string | null;
  /** False when the sign-in expired and the channel has to be reconnected. */
  connected: boolean;
}

export interface YoutubeChannels {
  channels: YoutubeChannel[];
  /** Why the account of an older version could not be carried over yet. */
  warning: string | null;
}

export interface YoutubeUpload {
  /** "4k"; uploads made by an older build may say "1080p". */
  resolution: string;
  video_id: string | null;
  privacy: string | null;
  publish_at: string | null;
  subtitles: "none" | "uploaded" | "pending";
  captions_error: string | null;
  unfinished: boolean;
  already_uploaded: boolean;
}

export type YoutubeChannelUpload = YoutubeUpload & { channel_id: string };

export interface YoutubeInfo {
  uploading: boolean;
  form: YoutubeForm;
  video: string | null;
  uploads: YoutubeChannelUpload[];
}

export type YoutubeDraft = Pick<YoutubeForm, "title" | "description" | "tags">;

export interface YoutubeProgress {
  story_id: number;
  percent: number;
}

export const api = {
  getYoutube: (storyId: number) => invoke<YoutubeInfo>("get_youtube", { storyId }),
  forgetLegacyYoutube: () => invoke<void>("forget_legacy_youtube"),
  youtubeChannels: () => invoke<YoutubeChannels>("youtube_channels"),
  connectYoutube: () => invoke<Omit<YoutubeChannel, "connected">>("connect_youtube"),
  cancelYoutubeConnect: () => invoke<void>("cancel_youtube_connect"),
  disconnectYoutube: (channelId: string) => invoke<void>("disconnect_youtube", { channelId }),
  saveYoutubeForm: (storyId: number, form: YoutubeForm) =>
    invoke<void>("save_youtube_form", { storyId, form }),
  draftYoutube: (storyId: number) => invoke<YoutubeDraft>("draft_youtube", { storyId }),
  uploadYoutube: (storyId: number, form: YoutubeForm, again: boolean) =>
    invoke<YoutubeUpload | null>("upload_youtube", { storyId, form, again }),
  cancelYoutubeUpload: () => invoke<void>("cancel_youtube_upload"),
  retryYoutubeCaptions: (storyId: number, channelId: string) =>
    invoke<YoutubeUpload>("retry_youtube_captions", { storyId, channelId }),
  listStories: () => invoke<Story[]>("list_stories"),
  createStory: (title: string, text: string) =>
    invoke<Story>("create_story", { title, text }),
  deleteStory: (id: number) => invoke<void>("delete_story", { id }),
  getStory: (id: number) => invoke<StoryDetail>("get_story", { id }),
  listProfiles: () => invoke<ProfilesInfo>("list_profiles"),
  setStorySettings: (id: number, settings: StorySettings) =>
    invoke<StoryDetail>("set_story_settings", {
      id,
      targetMinutes: settings.target_minutes,
      ageGroup: settings.age_group,
      profile: settings.profile,
    }),
  getModels: () => invoke<ModelsInfo>("get_models"),
  setModel: (id: string) => invoke<void>("set_model", { id }),
  acceptProposal: (messageId: number) =>
    invoke<StoryDetail>("accept_proposal", { messageId }),
  rejectProposal: (messageId: number) =>
    invoke<StoryDetail>("reject_proposal", { messageId }),
  listRevisions: (storyId: number) => invoke<Revision[]>("list_revisions", { storyId }),
  restoreRevision: (revisionId: number) =>
    invoke<StoryDetail>("restore_revision", { revisionId }),
  getMedia: (storyId: number) => invoke<MediaInfo>("get_media", { storyId }),
  setImageModel: (storyId: number, modelId: string) =>
    invoke<MediaInfo>("set_image_model", { storyId, modelId }),
  setVoice: (storyId: number, voiceId: string) =>
    invoke<MediaInfo>("set_voice", { storyId, voiceId }),
  requestAsset: (storyId: number, position: number, kind: AssetKind) =>
    invoke<MediaInfo>("request_asset", { storyId, position, kind }),
  requestMissingAssets: (storyId: number) =>
    invoke<MediaInfo>("request_missing_assets", { storyId }),
  cancelRequest: (storyId: number, position: number, kind: AssetKind) =>
    invoke<MediaInfo>("cancel_request", { storyId, position, kind }),
  cancelQueue: (storyId: number) => invoke<MediaInfo>("cancel_queue", { storyId }),
  /** Prices queued narration; one scene when `position` is given, else every unpriced one. Free. */
  estimateAudioQueue: (storyId: number, from?: number, to?: number) =>
    invoke<MediaInfo>("estimate_audio_queue", { storyId, from: from ?? null, to: to ?? null }),
  /** Generates priced narration (one scene or all). Spends credits. */
  generateAudioQueue: (storyId: number, from?: number, to?: number) =>
    invoke<MediaInfo>("generate_audio_queue", { storyId, from: from ?? null, to: to ?? null }),
  /** Same for images; generation wires scene 0 and the scene's own reference in. Spends credits. */
  estimateImageQueue: (storyId: number, from?: number, to?: number) =>
    invoke<MediaInfo>("estimate_image_queue", { storyId, from: from ?? null, to: to ?? null }),
  generateImageQueue: (storyId: number, from?: number, to?: number) =>
    invoke<MediaInfo>("generate_image_queue", { storyId, from: from ?? null, to: to ?? null }),
  /** Fetches the results of generations that were paid for but not delivered yet. Free. */
  collectMedia: (storyId: number, kind: AssetKind) => invoke<MediaInfo>("collect_media", { storyId, kind }),
  /** Gives up on a paid generation that never arrived, so it can be generated (and paid) again. */
  abandonGeneration: (storyId: number, position: number, kind: AssetKind) =>
    invoke<MediaInfo>("abandon_generation", { storyId, position, kind }),
  /** Upscayl + ffmpeg: writes 4K copies next to the finished image(s); one scene, or all when no position. */
  upscaleImages: (storyId: number, position?: number) =>
    invoke<MediaInfo>("upscale_images", { storyId, position: position ?? null }),
  /** Joins all scenes (zoom/pan clip + narration, fades) into one video. Long: minutes to tens of minutes. */
  exportVideo: (storyId: number) =>
    invoke<MediaInfo>("export_video", { storyId }),
  cancelExport: () => invoke<void>("cancel_export"),
  setSceneReference: (storyId: number, position: number, fileName: string, bytes: number[]) =>
    invoke<MediaInfo>("set_scene_reference", { storyId, position, fileName, bytes }),
  clearSceneReference: (storyId: number, position: number) =>
    invoke<MediaInfo>("clear_scene_reference", { storyId, position }),
  addCharacter: (storyId: number, name: string, fileName: string, bytes: number[]) =>
    invoke<MediaInfo>("add_character", { storyId, name, fileName, bytes }),
  /** Generates the character picture with the story's image model. Spends credits. */
  generateCharacter: (storyId: number, name: string, prompt: string) =>
    invoke<MediaInfo>("generate_character", { storyId, name, prompt }),
  removeCharacter: (storyId: number, characterId: number) =>
    invoke<MediaInfo>("remove_character", { storyId, characterId }),
  setSceneCharacter: (storyId: number, position: number, characterId: number, included: boolean) =>
    invoke<MediaInfo>("set_scene_character", { storyId, position, characterId, included }),
  sendMessage: (id: number, text: string) =>
    invoke<StoryDetail>("send_message", { id, text }),
};
