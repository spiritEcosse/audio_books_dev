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

export interface Message {
  id: number;
  role: "user" | "assistant";
  content: string;
  scenes_changed: boolean;
}

export interface StoryDetail {
  story: Story;
  scenes: Scene[];
  messages: Message[];
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
  getModels: () => invoke<ModelsInfo>("get_models"),
  setModel: (id: string) => invoke<void>("set_model", { id }),
  sendMessage: (id: number, text: string) =>
    invoke<StoryDetail>("send_message", { id, text }),
};
