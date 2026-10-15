"""The Jupyter widget: its project stays in step with the session both ways."""

import json

import pytest

pytest.importorskip("anywidget")

import rustgis  # noqa: E402

POINT = {
    "type": "FeatureCollection",
    "features": [{"type": "Feature", "properties": {"name": "A"}, "geometry": {"type": "Point", "coordinates": [10, 50]}}],
}


def from_app(m, text):
    """Delivers a message as the app's frontend would.

    Args:
        m: The widget.
        text: The project text the app sends.
    """
    m._on_app(m, {"type": "project", "text": text}, [])


def test_python_changes_reach_the_app():
    m = rustgis.Map()
    assert m._src.endswith("?embed")
    m.add_geojson(POINT, name="Sites")
    project = json.loads(m._project)
    assert [layer["name"] for layer in project["maps"][project["active_map"]]["layers"]] == ["Sites"]
    before = m._project
    m.set_center(10, 50, scale=1_000_000)
    assert m._project != before


def test_app_changes_reach_the_session():
    m = rustgis.Map()
    other = rustgis.Session()
    other.add_geojson(POINT, name="Drawn in the app")
    text = other.project_json()
    from_app(m, text)
    assert [layer["name"] for layer in m.layers()] == ["Drawn in the app"]
    # The app's own text is kept, so it isn't sent back to it.
    assert m._project == text
    # Opening it isn't reported.
    assert m.notices() == []


def test_a_bad_project_from_the_app_is_a_notice_not_a_crash():
    m = rustgis.Map()
    m.add_geojson(POINT, name="Sites")
    from_app(m, "not a project")
    assert [layer["name"] for layer in m.layers()] == ["Sites"]
    assert any(error for _, error in m.notices())
    m._on_app(m, {"type": "result", "ok": False, "error": "unknown command"}, [])
    m._on_app(m, "junk", [])
    assert m.notices() == [("unknown command", True)]


def test_options_and_local_files(tmp_path):
    path = tmp_path / "sites.geojson"
    path.write_text(json.dumps(POINT))
    m = rustgis.Map(data=[path], lang="de", url="http://127.0.0.1:8772/", height="400px")
    assert m._src == "http://127.0.0.1:8772/?embed&lang=de"
    assert m.height == "400px"
    # A small file travels with the project: the app in the browser can't read the path.
    assert str(path) not in m._project
    assert m.layers()[0]["features"] == 1


def test_add_data_parameters_can_override_the_file_name(tmp_path):
    path = tmp_path / "sites.geojson"
    path.write_text(json.dumps(POINT))
    m = rustgis.Map()
    m.add_data(path, name="Renamed", fileName="other.geojson")
    assert [layer["name"] for layer in m.layers()] == ["Renamed"]
