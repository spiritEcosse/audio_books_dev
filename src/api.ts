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
  sendMessage: (id: number, text: string) =>
    invoke<StoryDetail>("send_message", { id, text }),
};
