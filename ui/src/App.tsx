import "./styles.css";
import { Main } from "./views/Main";
import { Widget } from "./views/Widget";

/** One bundle serves both windows; the shell selects the view with `?view=`. */
export function App() {
  const view = new URLSearchParams(window.location.search).get("view");
  return view === "widget" ? <Widget /> : <Main />;
}
