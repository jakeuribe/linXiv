import arxiv
import os
import sys
file = "obsidian_vault\\arxivVault\\table_test.md"


def main():
    
    # Access the argument using args.filename
    print(f"Received filename: {file}")
    filename = file
    try:
        with open(filename, "r", encoding="utf-8") as f:
            content = f.read()
            print(content)
    except FileNotFoundError:
        print(f"Error: The file '{filename}' was not found.")

    # 2. Define your data (You would normally fetch this from arXiv)
    title = "A numerical study of the spatial coherence of light"
    url = "https://arxiv.org/abs/2408.10975v1"
    authors = ["Deniz Yavuz", "Anirudh Yadav", "David Gold", "Mark Saffman"]
    tags = ["clippings", "research", "clipping"]
    date = "2025-11-20"
    
    # 3. Format the YAML string
    # We use a loop to format the list of authors into "  - [[Name]]"
    author_list = "\n".join([f'  - "[[{name}]]"' for name in authors])
    tag_list = "\n".join([f'- {tag}' for tag in tags])

    try:
        with open("table_format.md", "r", encoding="utf-8") as f:
            template = f.read()
            final_content = template.format(
                title=title,
                url=url,
                author_list=author_list,
                date=date,
                tag_list=tag_list
            )
            with open(filename, "w", encoding="utf-8") as f:
                f.write(final_content)

    except FileNotFoundError:
        print("Error: Could not find 'table_format.md'")
        sys.exit(1)


    # 4. Write to the file
    

    print(f"Successfully created: {filename}")

if __name__ == "__main__":
    main()