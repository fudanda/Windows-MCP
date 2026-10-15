import { api, connectionChanged, type Config, type Snapshot } from "./api";

export interface ServiceStartup {
  controller: AbortController;
  promise: Promise<void>;
}

export async function startServices(
  snapshot: Snapshot,
  config: Config,
  update: (snapshot: Snapshot) => void,
  signal?: AbortSignal,
): Promise<boolean> {
  signal?.throwIfAborted();
  let current = snapshot;
  if (["starting", "stopping"].includes(current.phase))
    throw { code: "already_running" };
  if (current.phase !== "running" && connectionChanged(config, current.config)) {
    current = await api.save(config);
    update(current);
  }
  signal?.throwIfAborted();
  try {
    current = await api.startAll(current.generation);
  } catch (error) {
    // A transfer failure can leave MCP running. Refresh that partial result.
    const latest = await api.snapshot().catch(() => null);
    if (latest) update(latest);
    throw error;
  }
  update(current);
  signal?.throwIfAborted();
  if (current.error) throw current.error;
  // A native stop from the tray may have cancelled this request.
  return current.phase === "running";
}

export async function stopServices(
  update: (snapshot: Snapshot) => void,
  pending?: ServiceStartup | null,
): Promise<void> {
  pending?.controller.abort();
  const [result] = await Promise.allSettled([
    api.stopAll(),
    pending?.promise ?? Promise.resolve(),
  ]);
  if (result.status === "rejected") throw result.reason;
  update(result.value);
  if (result.value.phase !== "stopped")
    throw { code: "services_stop_failed", detail: result.value.error?.detail ?? "" };
}
