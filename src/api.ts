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
  error: string | null;
}

export interface SceneReference {
  position: number;
  file_path: string;
}

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
}

export interface ModelOption {
  id: string;
  label: string;
}

export interface ModelsInfo {
  options: ModelOption[];
  selected: string;
}

export const api = {
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
  setSceneReference: (storyId: number, position: number, fileName: string, bytes: number[]) =>
    invoke<MediaInfo>("set_scene_reference", { storyId, position, fileName, bytes }),
  clearSceneReference: (storyId: number, position: number) =>
    invoke<MediaInfo>("clear_scene_reference", { storyId, position }),
  sendMessage: (id: number, text: string) =>
    invoke<StoryDetail>("send_message", { id, text }),
};
