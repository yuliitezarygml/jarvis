import { invoke } from "@tauri-apps/api/core";

export { isTauri } from "@tauri-apps/api/core";

export interface AppInfo {
  name: string;
  version: string;
  os: string;
  arch: string;
}

export function greet(name: string): Promise<string> {
  return invoke<string>("greet", { name });
}

export function getAppInfo(): Promise<AppInfo> {
  return invoke<AppInfo>("get_app_info");
}
