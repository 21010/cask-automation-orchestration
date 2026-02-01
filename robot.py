import os
from robocorp.tasks import task


@task
def my_task(name, age):
    print(f"Hello {name} you are {age} years old")
    print("--------------------------------")
    print(f"Key from Env: {os.getenv('API_KEY')}")
    print("--------------------------------")
