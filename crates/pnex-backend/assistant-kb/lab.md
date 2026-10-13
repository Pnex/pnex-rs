---
id: lab
title: Vision Lab — teach PneX your own objects
kind: feature
pages: /lab, /lab/:id
nodes: vision_detect
err_codes: camera-unknown, camera-export-empty, ml-model-load-failed, ml-model-inference-failed, lab-project-not-found, lab-image-not-found, lab-version-not-found, lab-class-in-use, lab-annotations-conflict, lab-box-invalid, lab-asset-not-image, lab-dataset-empty
tools: 
tags: recording, enregistrement, video frames, images de vidéo, pre-annotation, pré-annotation, suggest, proposer, vision lab, lab, dataset, annotation, annoter, boîte, bounding box, custom model, modèle personnalisé, reconnaissance d'objets, object recognition, training, entraînement, coco, yolox
---
Vision Lab (Media › Vision Lab) builds the dataset of a custom object detector: name the classes ("red_valve", "safety_helmet"…), add pictures from the media library, draw a box around every object, then freeze a dataset version and download it in COCO format.

## What you can do
- **New project**: a name and the list of classes. Classes can be added or renamed later; a class that still has boxes cannot be removed.
- **Add images**: pick pictures already uploaded in **Media**. Every import is one *source*: all its images go either to training or to validation, never split between both, so near-identical pictures never inflate the scores.
- **From a recording**: take one picture every N seconds from what a camera recorded (pick the camera, the day, the start and end times; at most one hour and 200 pictures per import). The pictures land in the media library and in the project as one source, so a whole video stays on the same side of training and validation. Only device cameras with a **Video recording** flow have recordings.
- **Annotate**: select an image, pick a class (keys 1 to 9), drag a box around each object. Click a box to select it, **Delete** removes it. **No object in this image** marks a negative example (it teaches the model what is *not* the object). Every change is saved at once.
- **Pre-annotate**: pick a detection model of **Data › Models** (for example the YOLOX COCO model) and **Suggest boxes**: the model proposes dashed boxes. A proposal whose label is a class name of the project (person, car, dog… or your own trained classes) takes that class; **Keep the matching proposals** keeps them all, a click keeps one (a grey proposal takes the current class), **Dismiss** drops the rest. Nothing is saved until you keep a box. Once a first custom model exists, use it here to annotate the next pictures faster.
- **Statistics**: boxes and images per class, train / validation counts, and warnings: fewer than 20 annotated images for a class, fewer than 5 validation images, no negative example. They are advice, never a blocker.
- **Freeze a version**: copies the annotated images and their boxes into a numbered, read-only dataset version; later edits never change it.
- **Export**: downloads a version as a zip (images + COCO annotations for train and validation), ready for a YOLOX training.

## Using the trained model
Training inside PneX is not available yet: train the exported dataset with YOLOX (Apache-2.0), export it to ONNX, then register the file in **Data › Models** (it is checked on upload) and pick it in an **Object detection** node.

## Good to know
- Annotate the object in many situations: angles, distances, light, partly hidden. Twenty varied pictures beat a hundred identical ones.
- A box should hug the visible part of the object, including when it touches the image border.
- Two people editing the same image at the same time: the second save is refused ("changed meanwhile"); reload the image and redo the change.
- Only Owners, Admins and Members change a project; Viewers can browse and export.
