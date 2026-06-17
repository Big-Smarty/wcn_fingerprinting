import { invoke } from "@tauri-apps/api/core";
import floorplanUrl from "./assets/WLAN_AP_in-EG_C-Bau.jpg";

type Orientation = "u" | "d" | "l" | "r";

interface FingerprintSummary {
  pose: string;
  updatedAt: string;
  sampleCount: number;
  networkCount: number;
  scanThrottleEnabled: boolean | null;
}

interface BackupResult {
  filename: string;
  bytes: number;
  uri?: string | null;
  path?: string | null;
}

const columns = ["a", "b", "c", "d", "e"];
const rows = ["0", "1", "2", "3", "4", "5"];

let selectedCell: string | null = null;
let selectedOrientation: Orientation | null = null;
let busy = false;

const floorplan = document.querySelector<HTMLImageElement>("#floorplan");
const grid = document.querySelector<HTMLDivElement>("#grid");
const selectedCellEl = document.querySelector<HTMLElement>("#selected-cell");
const selectedOrientationEl = document.querySelector<HTMLElement>("#selected-orientation");
const startButton = document.querySelector<HTMLButtonElement>("#start-button");
const backupButton = document.querySelector<HTMLButtonElement>("#backup-button");
const message = document.querySelector<HTMLElement>("#message");
const orientationButtons = Array.from(
  document.querySelectorAll<HTMLButtonElement>(".orientation-button"),
);

function requireElement<T extends Element>(element: T | null, name: string): T {
  if (!element) {
    throw new Error(`Missing required element: ${name}`);
  }
  return element;
}

function setMessage(text: string, tone: "idle" | "success" | "error" = "idle") {
  const el = requireElement(message, "message");
  el.textContent = text;
  el.dataset.tone = tone;
}

function setBusy(nextBusy: boolean) {
  busy = nextBusy;
  updateControls();
}

function updateControls() {
  requireElement(selectedCellEl, "selected-cell").textContent = selectedCell ?? "-";
  requireElement(selectedOrientationEl, "selected-orientation").textContent =
    selectedOrientation?.toUpperCase() ?? "-";

  for (const button of orientationButtons) {
    button.classList.toggle("selected", button.dataset.orientation === selectedOrientation);
  }

  for (const button of Array.from(document.querySelectorAll<HTMLButtonElement>(".cell-button"))) {
    button.classList.toggle("selected", button.dataset.cell === selectedCell);
  }

  const canStart = selectedCell !== null && selectedOrientation !== null && !busy;
  requireElement(startButton, "start-button").disabled = !canStart;
  requireElement(backupButton, "backup-button").disabled = busy;
}

function buildGrid() {
  const target = requireElement(grid, "grid");
  target.innerHTML = "";

  for (const row of rows) {
    for (const column of columns) {
      const cell = `${column}${row}`;
      const button = document.createElement("button");
      button.type = "button";
      button.className = "cell-button";
      button.dataset.cell = cell;
      button.textContent = cell;
      button.setAttribute("aria-label", `Select cell ${cell}`);
      button.addEventListener("click", () => {
        selectedCell = cell;
        setMessage(`Selected ${cell}.`);
        updateControls();
      });
      target.appendChild(button);
    }
  }
}

async function startFingerprinting() {
  if (!selectedCell || !selectedOrientation || busy) {
    return;
  }

  setBusy(true);
  setMessage(`Scanning ${selectedCell}${selectedOrientation}... keep the phone steady.`);

  try {
    const result = await invoke<FingerprintSummary>("start_fingerprinting", {
      cell: selectedCell,
      orientation: selectedOrientation,
    });

    const throttleNote =
      result.scanThrottleEnabled === true
        ? " Wi-Fi scan throttling is enabled on this phone."
        : "";
    setMessage(
      `Saved ${result.pose}: ${result.networkCount} BSSID(s), ${result.sampleCount} fresh samples.${throttleNote}`,
      "success",
    );
  } catch (error) {
    setMessage(String(error), "error");
  } finally {
    setBusy(false);
  }
}

async function saveBackup() {
  if (busy) {
    return;
  }

  setBusy(true);
  setMessage("Preparing database backup...");

  try {
    const result = await invoke<BackupResult>("save_database_backup");
    const target = result.uri ?? result.path ?? result.filename;
    setMessage(`Saved ${result.filename} (${result.bytes} bytes) to ${target}.`, "success");
  } catch (error) {
    setMessage(String(error), "error");
  } finally {
    setBusy(false);
  }
}

window.addEventListener("DOMContentLoaded", () => {
  requireElement(floorplan, "floorplan").src = floorplanUrl;
  buildGrid();

  for (const button of orientationButtons) {
    button.addEventListener("click", () => {
      selectedOrientation = button.dataset.orientation as Orientation;
      setMessage(`Facing ${selectedOrientation.toUpperCase()} selected.`);
      updateControls();
    });
  }

  requireElement(startButton, "start-button").addEventListener("click", startFingerprinting);
  requireElement(backupButton, "backup-button").addEventListener("click", saveBackup);

  setMessage("Select a cell and orientation.");
  updateControls();
});
