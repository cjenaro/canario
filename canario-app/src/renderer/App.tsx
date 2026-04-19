// Root App component — routes between main window and overlay based on hash
import { AppPage } from "./pages/AppPage";
import { OverlayPage } from "./pages/OverlayPage";
import { AppProvider } from "./state/context";

function isOverlay(): boolean {
  return window.location.hash === "#overlay";
}

export default function App() {
  return (
    <AppProvider>
      {isOverlay() ? <OverlayPage /> : <AppPage />}
    </AppProvider>
  );
}
