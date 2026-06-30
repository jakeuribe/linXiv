import { lazy } from "react";
import { createBrowserRouter, RouterProvider, useParams } from "react-router-dom";
import AppShell from "./components/layout/AppShell";

const HomePage = lazy(() => import("./pages/HomePage"));
const LibraryPage = lazy(() => import("./pages/LibraryPage"));
const PaperDetailPage = lazy(() => import("./pages/PaperDetailPage"));
const ProjectsPage = lazy(() => import("./pages/ProjectsPage"));
const ProjectDetailPage = lazy(() => import("./pages/ProjectDetailPage"));
const DoiPage = lazy(() => import("./pages/DoiPage"));
const SearchPage = lazy(() => import("./pages/SearchPage"));
const SettingsPage = lazy(() => import("./pages/SettingsPage"));
const TagPage = lazy(() => import("./pages/TagPage"));
const AuthorPage = lazy(() => import("./pages/AuthorPage"));
const PdfPreviewPage = lazy(() => import("./pages/PdfPreviewPage"));
const NotePage = lazy(() => import("./pages/NotePage"));

// Forces a full remount of PaperDetailPage when sfk changes, so all
// useState initializers run fresh and no stale state drives incorrect queries.
function KeyedPaperDetailPage() {
  const { sfk } = useParams<{ sfk: string }>();
  return <PaperDetailPage key={sfk} />;
}

const router = createBrowserRouter([
  {
    path: "/",
    element: <AppShell />,
    children: [
      { index: true, element: <HomePage /> },
      { path: "library", element: <LibraryPage /> },
      { path: "library/:sfk", element: <KeyedPaperDetailPage /> },
      { path: "projects", element: <ProjectsPage /> },
      { path: "projects/:id", element: <ProjectDetailPage /> },
      { path: "graph", element: null },
      { path: "editor", element: null },
      { path: "search", element: <SearchPage /> },
      { path: "tags", element: <TagPage /> },
      { path: "tags/:label", element: <TagPage /> },
      { path: "authors", element: <AuthorPage /> },
      { path: "authors/:id", element: <AuthorPage /> },
      { path: "notes", element: null },
      { path: "notes/:id", element: <NotePage /> },
      { path: "doi", element: <DoiPage /> },
      { path: "settings", element: <SettingsPage /> },
      { path: "pdf-preview", element: <PdfPreviewPage /> },
    ],
  },
]);

export default function App() {
  return <RouterProvider router={router} />;
}
