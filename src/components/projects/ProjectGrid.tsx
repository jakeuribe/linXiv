import { useNavigate } from "react-router";
import type { Project } from "../../types/api";
import { ProjectCard } from "./ProjectCard";

export function ProjectGrid({ projects, className }: { projects: Project[]; className: string }) {
  const navigate = useNavigate();
  return (
    <div className={className}>
      {projects.map((project) => (
        <ProjectCard
          key={project.id}
          project={project}
          onClick={() => navigate(`/projects/${project.id}`)}
        />
      ))}
    </div>
  );
}
