import "./styles.css";
import { DetectionPrompt } from "./views/DetectionPrompt";
import { Main } from "./views/Main";
import { Overlay } from "./views/Overlay";
import { Widget } from "./views/Widget";

/** One bundle serves every window; the shell selects the view with `?view=`. */
export function App() {
  const view = new URLSearchParams(window.location.search).get("view");
  if (view === "widget") return <Widget />;
  if (view === "overlay") return <Overlay />;
  if (view === "detection") return <DetectionPrompt />;
  return <Main />;
}
