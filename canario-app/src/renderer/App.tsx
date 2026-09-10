// Root App component — routes between main window and overlay based on hash
import { createSignal, onMount, Show } from "solid-js";
import { AppPage } from "./pages/AppPage";
import { OnboardingPage } from "./pages/OnboardingPage";
import { OverlayPage } from "./pages/OverlayPage";
import { AppProvider, useAppState } from "./state/context";

function isOverlay(): boolean {
  return window.location.hash === "#overlay";
}

// Routes the main (settings) window: first launch (or a re-run from
// Settings → About) puts the machine into `onboarding` and shows the
// wizard; otherwise the normal settings page.
function MainRouter() {
  const machine = useAppState();
  const [checked, setChecked] = createSignal(false);

  onMount(async () => {
    try {
      const completed = await window.canario?.getOnboardingCompleted?.();
      if (!completed) {
        machine.send({ type: "START_ONBOARDING" });
      }
    } catch {
      // Not running in Electron — default to the settings page
    }
    setChecked(true);
  });

  return (
    <Show when={checked()}>
      <Show when={machine.state().status === "onboarding"} fallback={<AppPage />}>
        <OnboardingPage />
      </Show>
    </Show>
  );
}

export default function App() {
  return (
    <AppProvider>
      {isOverlay() ? <OverlayPage /> : <MainRouter />}
    </AppProvider>
  );
}
