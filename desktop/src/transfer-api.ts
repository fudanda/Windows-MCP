import { invoke } from "@tauri-apps/api/core";
export interface Peer {
  id: string;
  name: string;
  url: string;
  token: string;
}
export interface TransferSettings {
  port: number;
  lanEnabled: boolean;
  sharedDir: string;
  token: string;
  peers: Peer[];
}
export interface TransferTask {
  id: string;
  direction: "send" | "receive" | "download";
  peer?: string;
  name: string;
  status:
    | "preparing"
    | "transferring"
    | "verifying"
    | "completed"
    | "failed"
    | "cancelled"
    | "cancelling";
  totalBytes: number;
  transferredBytes: number;
  fileCount?: number;
  completedFiles?: number;
  startedAt: number;
  finishedAt: number | null;
  error: { code: string; detail?: string } | null;
  destination?: string | null;
}
export interface TransferSnapshot {
  settings: TransferSettings;
  running: boolean;
  tasks: TransferTask[];
  engine: string;
}
export const transferApi = {
  snapshot: () => invoke<TransferSnapshot>("transfer_snapshot"),
  command: <T = TransferSnapshot>(
    action: string,
    extra: Record<string, unknown> = {},
  ) => invoke<T>("transfer_command", { command: { action, ...extra } }),
};
export const fileUrl = (path: string, query = "") =>
  `/files/${path.split("/").filter(Boolean).map(encodeURIComponent).join("/")}${query}`;
export async function webRequest<T>(
  url: string,
  init: RequestInit = {},
): Promise<T> {
  const response = await fetch(url, { ...init, credentials: "same-origin" });
  if (!response.ok) {
    const failure = await response
      .json()
      .catch(() => ({
        code: response.status === 401 ? "unauthorized" : "transfer_network",
      }));
    throw failure;
  }
  return response.json() as Promise<T>;
}
