from dotenv import load_dotenv
from config import ENV_PATH
load_dotenv(ENV_PATH)

from gui.app_shell import run_shell


def main() -> None:
    run_shell()


if __name__ == "__main__":
    main()
