import { beforeEach, describe, expect, it, vi } from "vitest";
import { api, preview, type Snapshot } from "./api";
import { startServices, stopServices } from "./services";

vi.mock("./api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./api")>();
  return { ...actual, api: { ...actual.api, save: vi.fn(), snapshot: vi.fn(), startAll: vi.fn(), stopAll: vi.fn() } };
});
const stopped: Snapshot = { ...preview, generation: 4, backendAvailable: true, config: { ...preview.config, token: "a".repeat(32) } };
const running: Snapshot = { ...stopped, phase: "running", generation: 5 };

beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(api.startAll).mockResolvedValue(running);
  vi.mocked(api.stopAll).mockResolvedValue(stopped);
  vi.mocked(api.snapshot).mockResolvedValue(stopped);
});
describe("shared native service controls", () => {
  it("saves pending settings and sends the saved generation to native startup", async () => {
    const config = { ...stopped.config, port: 8101 };
    const saved = { ...stopped, config, generation: 5 };
    vi.mocked(api.save).mockResolvedValue(saved);
    const update = vi.fn();
    expect(await startServices(stopped, config, update)).toBe(true);
    expect(api.save).toHaveBeenCalledWith(config);
    expect(api.startAll).toHaveBeenCalledWith(5);
    expect(update).toHaveBeenCalledWith(saved);
  });
  it("uses the same native combined command when MCP is already running", async () => {
    expect(await startServices(running, running.config, vi.fn())).toBe(true);
    expect(api.save).not.toHaveBeenCalled();
    expect(api.startAll).toHaveBeenCalledWith(running.generation);
  });
  it("does not queue startup if cancelled while saving settings", async () => {
    let finish!: (value: Snapshot) => void;
    vi.mocked(api.save).mockReturnValue(new Promise((resolve) => { finish = resolve; }));
    const controller = new AbortController();
    const start = startServices(stopped, { ...stopped.config, port: 8101 }, vi.fn(), controller.signal);
    const rejected = expect(start).rejects.toMatchObject({ name: "AbortError" });
    controller.abort();
    finish(stopped);
    await rejected;
    expect(api.startAll).not.toHaveBeenCalled();
  });
  it("accepts a tray cancellation without claiming startup succeeded", async () => {
    vi.mocked(api.startAll).mockResolvedValue(stopped);
    expect(await startServices(stopped, stopped.config, vi.fn())).toBe(false);
  });
  it("preserves native startup errors", async () => {
    vi.mocked(api.startAll).mockResolvedValue({ ...stopped, phase: "error", error: { code: "startup_timeout", detail: "timeout" } });
    await expect(startServices(stopped, stopped.config, vi.fn())).rejects.toMatchObject({ code: "startup_timeout" });
  });
  it("refreshes MCP after partial failure without replacing the actual error", async () => {
    const failure = { code: "transfer_start_failed", detail: "port in use" };
    vi.mocked(api.startAll).mockRejectedValue(failure);
    vi.mocked(api.snapshot).mockResolvedValue(running);
    const update = vi.fn();
    await expect(startServices(stopped, stopped.config, update)).rejects.toBe(failure);
    expect(update).toHaveBeenCalledWith(running);
  });
  it("stops native services immediately and drains a pending frontend save", async () => {
    let finish!: (value: Snapshot) => void;
    vi.mocked(api.save).mockReturnValue(new Promise((resolve) => { finish = resolve; }));
    const controller = new AbortController();
    const promise = startServices(stopped, { ...stopped.config, port: 8101 }, vi.fn(), controller.signal).then(() => {});
    const update = vi.fn();
    const stop = stopServices(update, { controller, promise });
    expect(controller.signal.aborted).toBe(true);
    expect(api.stopAll).toHaveBeenCalledTimes(1);
    finish(stopped);
    await stop;
    expect(api.startAll).not.toHaveBeenCalled();
    expect(update).toHaveBeenCalledWith(stopped);
  });
  it("does not claim shutdown succeeded when native shutdown rejects", async () => {
    vi.mocked(api.stopAll).mockRejectedValue({ code: "process_failed" });
    await expect(stopServices(vi.fn())).rejects.toMatchObject({ code: "process_failed" });
  });
  it("rejects a stop response that still reports MCP running", async () => {
    vi.mocked(api.stopAll).mockResolvedValue(running);
    await expect(stopServices(vi.fn())).rejects.toMatchObject({ code: "services_stop_failed" });
  });
});
