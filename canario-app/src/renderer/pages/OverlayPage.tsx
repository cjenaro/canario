// Overlay page — loaded in the overlay BrowserWindow
import { RecordingOverlay } from "../components/RecordingOverlay";

export function OverlayPage() {
  return (
    <div
      class="w-screen h-screen overflow-hidden"
      style={{ "background-color": "transparent" }}
    >
      <RecordingOverlay />
    </div>
  );
}
