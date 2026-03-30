import psycopg2
import os
from dotenv import load_dotenv

# Load environment variables
load_dotenv()
try:
    
    server_pw = os.getenv("POSTGRES_PASS")

    conn = psycopg2.connect(
        host="localhost",
        database="postgres",
        user="postgres",
        password=server_pw 
    )

    # Create a cursor to perform database operations
    cur = conn.cursor()

    # Execute a test query
    cur.execute("SELECT version();")

    # Retrieve query result
    db_version = cur.fetchone()
    print(f"Connected to: {db_version[0]}")

    # Close communication with the database
    cur.close()
    conn.close()

except Exception as e:
    print(f"Error: {e}")