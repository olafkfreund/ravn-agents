import { Navigate, Route, Routes } from "react-router-dom";
import { Layout } from "./components/Layout";
import { Events } from "./pages/Events";

export function App() {
  return (
    <Routes>
      <Route element={<Layout />}>
        <Route index element={<Navigate to="/events" replace />} />
        <Route path="/events" element={<Events />} />
        <Route path="*" element={<Navigate to="/events" replace />} />
      </Route>
    </Routes>
  );
}
