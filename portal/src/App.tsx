import { Navigate, Route, Routes } from "react-router-dom";
import { Layout } from "./components/Layout";
import { Events } from "./pages/Events";
import { Agents } from "./pages/Agents";
import { Topology } from "./pages/Topology";
import { Remediations } from "./pages/Remediations";

export function App() {
  return (
    <Routes>
      <Route element={<Layout />}>
        <Route index element={<Navigate to="/events" replace />} />
        <Route path="/events" element={<Events />} />
        <Route path="/agents" element={<Agents />} />
        <Route path="/topology" element={<Topology />} />
        <Route path="/remediations" element={<Remediations />} />
        <Route path="*" element={<Navigate to="/events" replace />} />
      </Route>
    </Routes>
  );
}
