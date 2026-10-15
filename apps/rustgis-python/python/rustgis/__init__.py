"""RustGIS from Python: the RustGIS engine in-process, headless.

Every action in RustGIS is a command with an id and JSON parameters, the same ids the ribbon,
the command line and the AI Assistant use. ``Session.run`` runs any of them; the other methods
are shortcuts for the common ones.

>>> import rustgis
>>> s = rustgis.Session(sample=True)
>>> s.run("gp.run", tool="buffer", params={"input": "Cities", "distance": 100})
>>> s.to_png("map.png")
>>> s.save("atlas.rgis")

In Jupyter, ``rustgis.Map`` shows the RustGIS app in the cell, in step with its session
(``pip install "rustgis[jupyter]"``).
"""

from __future__ import annotations

import json
import os
from typing import Any, Dict, Iterable, List, Optional, Tuple, Union

from ._rustgis import Engine as _Engine
from ._rustgis import RustGISError, __version__
from ._rustgis import load_plugins as _load_plugins

__all__ = ["Layer", "Map", "RustGISError", "Session", "__version__", "load_plugins"]

PathLike = Union[str, "os.PathLike[str]"]


def __getattr__(name: str) -> Any:
    """Imports ``Map`` on first use, so the package works without the notebook widget's
    dependencies.

    Args:
        name: The attribute asked for.

    Raises:
        ImportError: When ``Map`` is asked for and anywidget isn't installed.
        AttributeError: For any other name.
    """
    if name == "Map":
        try:
            from .widget import Map
        except ImportError as e:
            raise ImportError('rustgis.Map needs anywidget: pip install "rustgis[jupyter]"') from e
        return Map
    raise AttributeError(f"module 'rustgis' has no attribute {name!r}")


def load_plugins() -> List[str]:
    """Loads the plugins installed for the desktop app, so their commands and tools run here too.

    Returns:
        The problems found, one message per plugin that couldn't be loaded.
    """
    return list(_load_plugins())


class Layer:
    """A layer of a session's active map, by name.

    Attributes:
        session: The session the layer belongs to.
        name: The layer's name.
    """

    def __init__(self, session: "Session", name: str) -> None:
        """Refers to layer ``name`` of ``session``.

        Args:
            session: The session the layer belongs to.
            name: The layer's name (or its id, as text).
        """
        self.session = session
        self.name = name

    @property
    def info(self) -> Dict[str, Any]:
        """The layer's summary: id, type, feature count, fields, renderer, visibility.

        Raises:
            RustGISError: When the map has no layer of this name.
        """
        for layer in self.session.layers():
            if layer.get("name") == self.name or str(layer.get("id")) == self.name:
                return layer
        raise RustGISError(f"no layer named '{self.name}'")

    def to_geojson(self) -> Dict[str, Any]:
        """The layer's features as a GeoJSON FeatureCollection, in longitude and latitude.

        A large file read in place holds the features of the last view; run
        ``session.run("layer.loadAll", layer=name)`` first to get all of them.

        Returns:
            The FeatureCollection as a dict.
        """
        return json.loads(self.session._engine.layer_geojson(self.name))

    def to_geopandas(self) -> Any:
        """The layer as a ``geopandas.GeoDataFrame`` in EPSG:4326.

        Returns:
            A GeoDataFrame with the layer's features and attributes.

        Raises:
            ImportError: When geopandas isn't installed (``pip install rustgis[geopandas]``).
        """
        import geopandas as gpd

        collection = self.to_geojson()
        return gpd.GeoDataFrame.from_features(collection["features"], crs="EPSG:4326")

    def __repr__(self) -> str:
        return f"Layer({self.name!r})"


class Session:
    """A RustGIS project open in memory: its maps, layers, symbology, layouts and undo history.

    Args:
        project: A project file (``.rgis``) to open.
        sample: Open the World Atlas sample project.
        data: Files to add as layers (GeoJSON, CSV, Shapefile, GeoPackage, GeoParquet,
            FlatGeobuf, KML, GPX, GeoTIFF and more).
        plugins: Load the plugins installed for the desktop app first.

    Example:
        >>> s = rustgis.Session(data=["parcels.gpkg"])
        >>> s.layers()[0]["features"]
    """

    def __init__(
        self,
        project: Optional[PathLike] = None,
        *,
        sample: bool = False,
        data: Optional[Union[PathLike, Iterable[PathLike]]] = None,
        plugins: bool = False,
    ) -> None:
        """Starts a session, optionally opening a project, the sample and data files."""
        if plugins:
            load_plugins()
        self._engine = _Engine()
        if sample:
            self.run("project.sample")
        if project is not None:
            self.open(project)
        if data is not None:
            for path in [data] if isinstance(data, (str, os.PathLike)) else data:
                self.add_data(path)

    # Commands ------------------------------------------------------------------------------

    def run(self, command: str, params: Optional[Dict[str, Any]] = None, /, **kwargs: Any) -> Any:
        """Runs a RustGIS command by id.

        Parameters can be given as a dict, as keyword arguments, or both (keyword arguments win).
        ``Session.commands()`` lists every command with its parameters. The dict is positional
        only, so a command's own ``params`` parameter (``gp.run`` has one) can be a keyword:
        ``s.run("gp.run", tool="buffer", params={"input": "Cities", "distance": 100})``.

        Args:
            command: The command id, e.g. ``"layer.symbology"`` or ``"gp.run"``.
            params: The command's parameters, as a dict (positional only).
            **kwargs: Parameters as keyword arguments.

        Returns:
            The command's result, decoded from JSON (``None`` for commands that return nothing).

        Raises:
            RustGISError: When the command fails; the message says why.
        """
        merged: Dict[str, Any] = dict(params or {})
        merged.update(kwargs)
        try:
            encoded = json.dumps(merged, default=_jsonable, allow_nan=False) if merged else ""
        except ValueError as e:
            raise ValueError(f"{command}: parameters must be finite numbers, text, lists and dicts ({e})") from e
        text = self._engine.run(command, encoded)
        return json.loads(text)

    def commands(self) -> List[Dict[str, Any]]:
        """Every command: its id, label, ribbon location and parameters.

        Returns:
            One dict per command.
        """
        result = self.run("info.commands")
        return result.get("commands", result) if isinstance(result, dict) else result

    def tools(self) -> Any:
        """The geoprocessing tools, with their parameters.

        Returns:
            The tool list, as ``gp.tools`` returns it.
        """
        return self.run("gp.tools")

    def notices(self) -> List[Tuple[str, bool]]:
        """What commands have reported since the last call: warnings about a file, a tool's
        summary, and so on.

        Returns:
            ``(text, is_error)`` pairs, oldest first.
        """
        return list(self._engine.take_notices())

    def undo(self) -> Any:
        """Undoes the last change."""
        return self.run("edit.undo")

    def redo(self) -> Any:
        """Redoes the last undone change."""
        return self.run("edit.redo")

    # Projects ------------------------------------------------------------------------------

    def open(self, path: PathLike) -> Any:
        """Opens a project file (``.rgis``), replacing the current project.

        Args:
            path: The project file.
        """
        return self.run("project.open", path=os.fspath(path))

    def save(self, path: PathLike) -> Any:
        """Saves the project to ``path`` (``.rgis``); the app opens it as it is.

        Args:
            path: Where to save.
        """
        return self.run("project.saveAs", path=os.fspath(path))

    def project_json(self) -> str:
        """The project as ``.rgis`` JSON text, as :meth:`save` writes it.

        Returns:
            The JSON text; ``run("project.openText", text=...)`` opens it.
        """
        return self._engine.project_json()

    def info(self) -> Dict[str, Any]:
        """The project's summary: the active map, basemap, view and layers."""
        return self.run("info.project")

    # Layers --------------------------------------------------------------------------------

    def layers(self) -> List[Dict[str, Any]]:
        """The active map's layers, top first, each with its id, name, type, feature count,
        fields, renderer and visibility."""
        return list(self.info().get("layers", []))

    def layer(self, name: str) -> Layer:
        """Layer ``name`` of the active map.

        Args:
            name: The layer's name (or its id, as text).
        """
        return Layer(self, name)

    def add_data(self, path: PathLike, name: Optional[str] = None, **params: Any) -> Any:
        """Adds a file as a layer, or as a standalone table when it has no geometry.

        Args:
            path: The file: GeoJSON, CSV, Shapefile (or a zip of them), GeoPackage, GeoParquet,
                FlatGeobuf, KML/KMZ, GPX, GeoTIFF, ASCII grid, and the formats GDAL or DuckDB
                read when they are installed.
            name: The layer's name (the file's name when left out).
            **params: More ``layer.addData`` parameters, e.g. ``layer=`` for one table of a
                GeoPackage.
        """
        if name is not None:
            params["name"] = name
        return self.run("layer.addData", path=os.fspath(path), **params)

    def add_geojson(self, data: Union[Dict[str, Any], str, PathLike], name: str = "GeoJSON") -> Any:
        """Adds GeoJSON as a layer.

        Args:
            data: A GeoJSON dict, GeoJSON text, or the path of a GeoJSON file.
            name: The layer's name.
        """
        if isinstance(data, dict):
            text = json.dumps(data, default=_jsonable, allow_nan=False)
        elif isinstance(data, str) and data.lstrip().startswith("{"):
            text = data
        else:
            return self.add_data(data, name=name)
        return self.run("layer.addDataText", fileName=f"{name}.geojson", text=text, name=name)

    def add_geopandas(self, gdf: Any, name: str = "GeoDataFrame") -> Any:
        """Adds a ``geopandas.GeoDataFrame`` as a layer (reprojected to EPSG:4326 if needed).

        Args:
            gdf: The GeoDataFrame.
            name: The layer's name.
        """
        if getattr(gdf, "crs", None) is not None and not gdf.crs.equals("EPSG:4326"):
            gdf = gdf.to_crs("EPSG:4326")
        return self.add_geojson(gdf.to_json(), name=name)

    # Maps ----------------------------------------------------------------------------------

    def to_png(
        self,
        path: Optional[PathLike] = None,
        *,
        width: Optional[int] = None,
        height: Optional[int] = None,
        dpi: Optional[float] = None,
    ) -> bytes:
        """Renders the active map, with its labels, as a PNG.

        Args:
            path: Where to write the PNG; when left out only the bytes are returned.
            width: Width in pixels (the view's own when left out).
            height: Height in pixels.
            dpi: Resolution (96 when left out).

        Returns:
            The PNG bytes.
        """
        png = bytes(
            self._engine.render_png(
                None if width is None else float(width),
                None if height is None else float(height),
                None if dpi is None else float(dpi),
            )
        )
        if path is not None:
            with open(path, "wb") as f:
                f.write(png)
        return png

    def _repr_png_(self) -> bytes:
        """The map as a PNG, for Jupyter."""
        return self.to_png(width=960, height=540)

    def __repr__(self) -> str:
        try:
            names = [layer["name"] for layer in self.layers()]
        except RustGISError:
            names = []
        return f"Session(layers={names!r})"


def _jsonable(value: Any) -> Any:
    """Turns values JSON can't encode (paths, numpy numbers) into ones it can."""
    if isinstance(value, os.PathLike):
        return os.fspath(value)
    if hasattr(value, "item"):
        return value.item()
    if hasattr(value, "tolist"):
        return value.tolist()
    raise TypeError(f"{type(value).__name__} isn't JSON serialisable")
