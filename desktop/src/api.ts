import { invoke, isTauri } from "@tauri-apps/api/core";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import type { Language } from "./i18n";
export interface Config {
  language: Language;
  port: number;
  lanEnabled: boolean;
  token: string;
}
export interface Probe {
  serverName: string;
  serverVersion: string;
  toolCount: number;
  latencyMs: number;
}
export interface Snapshot {
  generation: number;
  config: Config;
  phase: "stopped" | "starting" | "running" | "stopping" | "error";
  pid: number | null;
  startedAt: number | null;
  error: { code: string; detail: string } | null;
  probe: Probe | null;
  logs: { timestamp: number; message: string }[];
  addresses: { name: string; ip: string }[];
  backendAvailable: boolean;
  configPath: string;
}
export const native = isTauri();
export const preview: Snapshot = {
  generation: 0,
  config: { language: "zh-CN", port: 8001, lanEnabled: false, token: "" },
  phase: "stopped",
  pid: null,
  startedAt: null,
  error: null,
  probe: null,
  logs: [],
  addresses: [],
  backendAvailable: false,
  configPath: "",
};
export const api = {
  snapshot: () =>
    native ? invoke<Snapshot>("get_snapshot") : Promise.resolve(preview),
  save: (config: Config) => invoke<Snapshot>("save_config", { config }),
  language: (language: Language) =>
    invoke<Snapshot>("set_language", { language }),
  generateToken: () => invoke<string>("generate_token"),
  start: (generation: number) =>
    invoke<Snapshot>("start_server", { generation }),
  stop: () => invoke<Snapshot>("stop_server"),
  test: () => invoke<Probe>("test_connection"),
  copy: (text: string) =>
    native ? writeText(text) : navigator.clipboard.writeText(text),
};
export function connectionConfig(
  config: Config,
  host: string,
  format: "toml" | "json",
  masked = false,
): string {
  const url = `http://${host}:${config.port}/mcp`;
  const token = masked ? "<YOUR_TOKEN>" : config.token;
  if (format === "toml")
    return `[mcp_servers.windows-mcp]\nurl = ${JSON.stringify(url)}\nhttp_headers = { Authorization = ${JSON.stringify(`Bearer ${token}`)} }\nstartup_timeout_sec = 120`;
  return JSON.stringify(
    {
      mcpServers: {
        "windows-mcp": { url, headers: { Authorization: `Bearer ${token}` } },
      },
    },
    null,
    2,
  );
}
export function connectionChanged(a: Config, b: Config): boolean {
  return (
    a.port !== b.port || a.lanEnabled !== b.lanEnabled || a.token !== b.token
  );
}
export function validConfig(config: Config): boolean {
  return (
    Number.isInteger(config.port) &&
    config.port >= 1 &&
    config.port <= 65535 &&
    /^[!-~]{32,256}$/.test(config.token)
  );
}
