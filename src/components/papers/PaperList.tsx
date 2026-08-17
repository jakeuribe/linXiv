import { useNavigate } from "react-router";
import type { Paper } from "../../types/api";
import { PaperCard } from "./PaperCard";

export function PaperList({ papers, className }: { papers: Paper[]; className: string }) {
  const navigate = useNavigate();
  return (
    <div className={className}>
      {papers.map((paper) => (
        <PaperCard
          key={paper.source_id}
          paper={paper}
          onNavigate={(sfk) => navigate(`/library/${sfk}`)}
        />
      ))}
    </div>
  );
}
