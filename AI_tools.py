import os
import re
from google import genai
from dotenv import load_dotenv

# Load environment variables
load_dotenv()

def generate_and_save_obsidian_tags(text_content, file_path):
    """
    Generates Obsidian-formatted tags using the new Google Gen AI SDK
    and appends them to a file.
    """
    key_name = "GENAI_API_KEY_TAG_GEN"
    api_key = os.getenv(key_name)
    if not api_key:
        print(f"Error: {key_name} not found.")
        return []

    # 1. Initialize Client (New Way)
    # The new SDK uses a client instance rather than global configuration
    client = genai.Client(api_key=api_key)

    prompt = f"""
    Analyze the following text and generate 3-5 relevant metadata tags.
    Format them strictly as Obsidian-style tags: start with a hash (#), no spaces.
    Use underscores (_) or hyphens (-) for multi-word concepts.
    Return ONLY the tags separated by spaces. Do not add numbering or explanations.

    Text:
    "{text_content}"
    """

    try:
        # 2. Generate Content (New Way)
        # client.models.generate_content replaces model.generate_content
        response = client.models.generate_content(
            model="gemini-2.0-flash",  # Or "gemini-1.5-flash"
            contents=prompt
        )
        raw_tags = response.text.strip().split()
    except Exception as e:
        print(f"API Error: {e}")
        return []

    
    valid_tags = []
    format_pattern = re.compile(r"^#[a-zA-Z0-9_\-/]+$")
    non_numeric_pattern = re.compile(r"[a-zA-Z_\-/]") 

    for tag in raw_tags:
        if not tag.startswith("#"):
            tag = "#" + tag
        
        if format_pattern.match(tag) and non_numeric_pattern.search(tag):
            valid_tags.append(tag)

    # 4. Append to File
    if valid_tags:
        try:
            with open(file_path, "a", encoding="utf-8") as f:
                f.write("\n" + " ".join(valid_tags))
            print(f"Success. Added: {valid_tags}")
        except IOError as e:
            print(f"File Error: {e}")
    else:
        print("No valid tags generated.")

    return valid_tags, file_path