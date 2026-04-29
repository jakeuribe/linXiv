from sqlite3 import connect, Connection
import os, glob
from typing import Iterable
from pathlib import Path
DB_PATH = str(Path(__file__).parent.parent.parent / "papers.db")
def init_db(db_path:str|None = None)->None:
    global DB_PATH
    if(db_path is not None):
        DB_PATH = db_path
    path_base = os.path.join(
        '/'.join(os.path.abspath(__file__).split('/')[:-1]),
        '/sql/'
    )
    
    conn = connect(DB_PATH)
    #Building Tables
    exec_paths(glob.glob(os.path.join(path_base,'/tables/*')), conn)
    #Building Views
    exec_paths(glob.glob(os.path.join(path_base,'/views/*')), conn)
    #Making_Indicies
    with open(os.path.join(path_base, 'INDICIES.SQL'), 'w') as file:
        SQL =file.read().split(";")
        IX_ER = set(map(lambda SQL: exec_str(SQL, conn), filter(lambda x: x.strip(' ') != '', SQL)))
        if(len(IX_ER)!= 1):
            for i in IX_ER:
                print(i)
    conn.close()
    return 
def exec_paths(paths:Iterable[str], conn:Connection)->None:
    tab_ers = set(map(lambda path: exec_at_path(path=path, conn = conn), paths))
    if(len(tab_ers) != 1):
        for er in tab_ers:
            print(er)
    pass
def exec_at_path(path:str, conn:Connection)->str:
    with open(path, 'r') as file:
        SQL = file.read()
    return exec_str(SQL, conn)

def exec_str(sql:str, conn:Connection)->str:
    er = None
    try:
        conn.execute(sql)
    except Exception as e:
        er = str(e)
    conn.commit()
    return er