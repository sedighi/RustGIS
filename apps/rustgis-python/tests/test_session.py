"""Tests for the rustgis Python package (run after `maturin develop`)."""

import json

import pytest

import rustgis


@pytest.fixture
def sample():
    """A session with the World Atlas sample open."""
    return rustgis.Session(sample=True)


def test_version_matches_the_engine():
    assert rustgis.__version__.count(".") >= 2


def test_sample_layers(sample):
    names = [layer["name"] for layer in sample.layers()]
    assert {"Cities", "Countries"} <= set(names)
    assert sample.layer("Cities").info["features"] > 200


def test_commands_and_tools_are_listed(sample):
    ids = {c["id"] for c in sample.commands()}
    assert {"layer.addData", "gp.run", "project.saveAs"} <= ids
    assert "buffer" in json.dumps(sample.tools())


def test_run_takes_keywords_including_a_params_keyword(sample):
    result = sample.run("gp.run", tool="buffer", params={"input": "Cities", "distance": 100})
    assert "layer" in result
    assert any("Buffer" in text for text, _ in sample.notices())
    assert sample.notices() == []
    # A dict works too, and keywords win over it.
    sample.run("layer.transparency", {"layer": "Countries", "value": 10}, value=40)
    assert any(l["name"] == "Countries" for l in sample.layers())


def test_errors_raise_rustgis_error(sample):
    with pytest.raises(rustgis.RustGISError, match="unknown command"):
        sample.run("no.such.command")
    with pytest.raises(rustgis.RustGISError):
        sample.layer("No such layer").to_geojson()


def test_parameters_must_be_json(sample):
    with pytest.raises(ValueError, match="finite"):
        sample.run("map.setScale", scale=float("nan"))


def test_geojson_round_trip(sample):
    collection = sample.layer("Cities").to_geojson()
    assert collection["type"] == "FeatureCollection" and len(collection["features"]) > 200
    big = {"type": "FeatureCollection", "features": [f for f in collection["features"] if f["properties"]["pop_max"] > 10_000_000]}
    sample.add_geojson(big, name="Megacities")
    assert sample.layer("Megacities").info["features"] == len(big["features"])
    sample.undo()
    assert "Megacities" not in [l["name"] for l in sample.layers()]


def test_png(sample, tmp_path):
    out = tmp_path / "map.png"
    png = sample.to_png(out, width=400, height=200)
    assert png[:4] == b"\x89PNG" and out.read_bytes() == png
    assert sample._repr_png_()[:4] == b"\x89PNG"


def test_projects_save_and_open(sample, tmp_path):
    path = tmp_path / "atlas.rgis"
    sample.run("gp.run", tool="buffer", params={"input": "Cities", "distance": 50})
    sample.save(path)
    reopened = rustgis.Session(path)
    assert [l["name"] for l in reopened.layers()] == [l["name"] for l in sample.layers()]


def test_data_files(tmp_path):
    path = tmp_path / "points.geojson"
    path.write_text(json.dumps({"type": "FeatureCollection", "features": [
        {"type": "Feature", "geometry": {"type": "Point", "coordinates": [114.17, 22.3]}, "properties": {"name": "Hong Kong"}},
        {"type": "Feature", "geometry": {"type": "Point", "coordinates": [113.26, 23.13]}, "properties": {"name": "Guangzhou"}},
    ]}))
    s = rustgis.Session(data=path)
    assert s.layers()[0]["features"] == 2


def test_geopandas_round_trip(sample):
    gpd = pytest.importorskip("geopandas")
    gdf = sample.layer("Cities").to_geopandas()
    assert isinstance(gdf, gpd.GeoDataFrame) and gdf.crs.to_epsg() == 4326
    sample.add_geopandas(gdf.head(5), name="Five")
    assert sample.layer("Five").info["features"] == 5


def test_a_session_is_usable_from_another_thread():
    """Jupyter delivers widget messages on a thread of its own."""
    import threading

    s = rustgis.Session(sample=True)
    seen = []
    t = threading.Thread(target=lambda: seen.append(len(s.layers())))
    t.start()
    t.join()
    assert seen == [len(s.layers())]


def test_project_json_reopens_as_the_same_project():
    s = rustgis.Session(sample=True)
    other = rustgis.Session()
    other.run("project.openText", text=s.project_json())
    assert [layer["name"] for layer in other.layers()] == [layer["name"] for layer in s.layers()]
