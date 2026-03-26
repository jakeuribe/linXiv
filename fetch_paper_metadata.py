import sys, os
import arxiv
from typing import Sequence, Generator, Iterable, Iterator
from AI_tools import generate_and_save_obsidian_tags
from db import init_db, save_papers

# The ID of the paper you want to fetch
paper_id = "2204.12985"

# Shared client — enforces arXiv's required 3-second delay between requests
_client = arxiv.Client(num_retries=2, delay_seconds=3.0)

def fetch_paper_metadata(paper_id:str, print_on: bool = False) -> arxiv.Result:
    search = arxiv.Search(id_list=[paper_id])
    paper = next(_client.results(search))
    
    if print_on:
        # Display Metadata
        print(f"Title:    {paper.title}")
        print(f"Date:     {paper.published.strftime('%Y-%m-%d')}")
        print(f"Authors:  {', '.join(author.name for author in paper.authors)}")
        print(f"Category: {paper.primary_category}")
        print(f"DOI:      {paper.doi}")
        print(f"PDF URL:  {paper.pdf_url}")
        print("-" * 30)
        print(f"Abstract:\n{paper.summary}")
    
    return paper

def search_papers(query: str, max_results: int = 10, print_on: bool = False) -> list[arxiv.Result]:
    search = arxiv.Search(query=query, max_results=max_results)
    papers = list(_client.results(search))

    if print_on:
        for paper in papers:
            print(f"Title:    {paper.title}")
            print(f"Date:     {paper.published.strftime('%Y-%m-%d')}")
            print(f"Authors:  {', '.join(author.name for author in paper.authors)}")
            print(f"Category: {paper.primary_category}")
            print("-" * 30)

    save_papers(papers)
    return papers

def gen_md_files(papers: list[arxiv.Result], additional_tags:None|Sequence[str] = None) -> None:
    
    for paper in papers:
        gen_md_file(paper, additional_tags=additional_tags)

def gen_md_file(paper: arxiv.Result, additional_tags:None|Sequence[str] = None, print_on: bool = False):
    # Access the argument using args.filename
    title:str = paper.title
    id:str = paper.entry_id.split('/')[-1]
    url:str = f"https://arxiv.org/abs/{id}"
    authors:list[str] = [author.name for author in paper.authors]
    tags:list[str] = ["clippings", "research", "clipping"]

    if additional_tags is not None:
        for s in additional_tags:
            tags.append(s)

    date = paper.published.strftime('%Y-%m-%d')
    filename = f"obsidian_vault\\arxivVault\\{id}.md"

    author_list = "\n".join([f'  - "[[{name}]]"' for name in authors])
    tag_list = "\n".join([f'- {tag}' for tag in tags])
    with open("./formats/table_format.md", "r", encoding="utf-8") as f:
        template = f.read()
        final_content = template.format(
            title=title,
            url=url,
            author_list=author_list,
            date=date,
            tag_list=tag_list
        )
    if print_on:
        print(final_content)
    with open(filename, "w", encoding="utf-8") as f:
        f.write(final_content)

if __name__ == "__main__":
    init_db()
    paper = fetch_paper_metadata(paper_id)
    
    # generated_tags, file_path = generate_and_save_obsidian_tags(text_content=paper.summary, file_path="paper_tags.md")

    gen_md_file(paper)

    search:str = "QEC"
    papers = search_papers(search, max_results=5)
    print(save_papers(papers=papers))
    gen_md_files(papers, additional_tags=[search])